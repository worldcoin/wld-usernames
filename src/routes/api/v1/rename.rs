use axum::Extension;
use axum_jsonschema::Json;
use http::StatusCode;
use idkit::session::VerificationLevel;
use tracing::{info_span, Instrument};

use crate::{
	blocklist::BlocklistExt,
	config::{ConfigExt, Db, DEVICE_USERNAME_REGEX, USERNAME_REGEX},
	types::{ErrorResponse, MovedAddress, Name, RenamePayload},
	verify,
};
use redis::{aio::ConnectionManager, AsyncCommands};

#[tracing::instrument(skip_all)]
#[allow(clippy::too_many_lines)] // TODO: refactor
#[allow(dependency_on_unit_never_type_fallback)]
pub async fn rename(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Extension(blocklist): BlocklistExt,
	Json(payload): Json<RenamePayload>,
) -> Result<StatusCode, ErrorResponse> {
	let Some(record) = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE username = $1",
		&payload.old_username
	)
	.fetch_optional(&db.read_write)
	.instrument(info_span!(
		"rename_check_existing",
		username = payload.old_username
	))
	.await?
	else {
		return Err(ErrorResponse::not_found("Username not found".to_string()));
	};

	if record.nullifier_hash != payload.nullifier_hash {
		return Err(ErrorResponse::unauthorized(
			"You can't update this name".to_string(),
		));
	}

	match verify::dev_portal_verify_proof(
		payload.into_proof(),
		config.wld_app_id.to_string(),
		"username",
		(&payload.old_username, &payload.new_username),
		config.developer_portal_url.clone(),
	)
	.await
	{
		Ok(()) => {},
		Err(verify::Error::Verification(e)) => {
			tracing::error!(
				"Rename Verification Error: {}, payload:{:?}",
				e.detail,
				payload
			);
			return Err(ErrorResponse::validation_error(e.detail));
		},
		Err(e) => {
			tracing::error!(
				"Rename Server Error: {}, payload:{:?}",
				e.to_string(),
				payload
			);
			return Err(ErrorResponse::server_error(
				"Failed to verify World ID proof".to_string(),
			));
		},
	};

	let username_regex = match payload.verification_level.0 {
		VerificationLevel::Orb => USERNAME_REGEX.clone(),
		VerificationLevel::Device => DEVICE_USERNAME_REGEX.clone(),
	};

	if !username_regex.is_match(&payload.new_username) {
		tracing::warn!(
			"Username does not match the required pattern, payload:{:?}",
			payload,
		);
		return Err(ErrorResponse::validation_error(
			"Username does not match the required pattern".to_string(),
		));
	}

	blocklist.ensure_valid(&payload.new_username).map_err(|e| {
		tracing::warn!("Blocklist error, payload:{:?}", payload);
		ErrorResponse::validation_error(e.to_string())
	})?;

	let uniqueness_check = sqlx::query!(
		"SELECT
            EXISTS(SELECT 1 FROM old_names where LOWER(new_username) = LOWER($1)) AS has_old_username,
            EXISTS(SELECT 1 FROM names WHERE LOWER(username) = LOWER($2) 
                UNION 
                SELECT 1 FROM old_names where LOWER(old_username) = LOWER($2) AND LOWER(new_username) != LOWER($1)
            ) AS username
        ",
		&payload.old_username,
		&payload.new_username,
	)
	.fetch_one(&db.read_write)
	.instrument(info_span!(
		"rename_uniqueness_check",
		old_username = payload.old_username,
		new_username = payload.new_username
	))
	.await?;

	if uniqueness_check.username.unwrap_or_default() {
		tracing::warn!("Username is already taken, payload:{:?}", payload);
		return Err(ErrorResponse::validation_error(
			"Username is already taken".to_string(),
		));
	};

	let mut tx = db.read_write.begin().await?;

	if uniqueness_check.has_old_username.unwrap_or_default() {
		sqlx::query!(
			"DELETE FROM old_names WHERE new_username = $1",
			&payload.old_username
		)
		.execute(&mut *tx)
		.instrument(info_span!(
			"rename_delete_old_name",
			username = payload.old_username
		))
		.await?;
	}

	let moved_address = sqlx::query_as!(
		MovedAddress,
		"UPDATE names SET username = $1 WHERE username = $2 RETURNING address",
		&payload.new_username,
		&payload.old_username,
	)
	.fetch_one(&mut *tx)
	.instrument(info_span!(
		"rename_update_name",
		old_username = payload.old_username,
		new_username = payload.new_username
	))
	.await?;

	sqlx::query!(
		"INSERT INTO old_names (old_username, new_username) VALUES ($1, $2)",
		&payload.old_username,
		&payload.new_username,
	)
	.execute(&mut *tx)
	.instrument(info_span!(
		"rename_insert_old_name",
		old_username = payload.old_username,
		new_username = payload.new_username
	))
	.await?;

	tx.commit().await?;

	let query_single_username_cache_key = format!("query_single:{}", payload.old_username);
	let query_single_address_cache_key = format!("query_single:{}", moved_address.address);

	redis
		.del::<_, String>(&query_single_username_cache_key)
		.await?;
	redis
		.del::<_, String>(&query_single_address_cache_key)
		.await?;

	Ok(StatusCode::OK)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Change your World App username to a new one.")
}
