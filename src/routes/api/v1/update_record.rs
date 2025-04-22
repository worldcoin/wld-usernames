use axum::{extract::Path, Extension};
use axum_jsonschema::Json;
use http::StatusCode;
use tracing::{info_span, Instrument};

use crate::{
	config::{ConfigExt, Db},
	types::{ErrorResponse, Name, UpdateUsernamePayload},
	verify,
};

#[allow(dependency_on_unit_never_type_fallback)]
pub async fn update_record(
	Path(username): Path<String>,
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Json(payload): Json<UpdateUsernamePayload>,
) -> Result<StatusCode, ErrorResponse> {
	let Some(record) = sqlx::query_as!(Name, "SELECT * FROM names WHERE username = $1", username)
		.fetch_optional(&db.read_write)
		.instrument(info_span!(
			"update_record_check_existing",
			username = username
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
		(
			&username,
			payload.address.to_checksum(None),
			payload
				.profile_picture_url
				.as_ref()
				.map(ToString::to_string)
				.unwrap_or_default(),
		),
		config.developer_portal_url.clone(),
	)
	.await
	{
		Ok(()) => {},
		Err(verify::Error::Verification(e)) => {
			tracing::error!(
				"Update Record Verification Error: {}, payload:{:?}",
				e.detail,
				payload
			);
			return Err(ErrorResponse::validation_error(e.detail));
		},
		Err(e) => {
			tracing::error!(
				"Update Record Server Error: {}, payload:{:?}",
				e.to_string(),
				payload
			);
			return Err(ErrorResponse::server_error(
				"Failed to verify World ID proof".to_string(),
			));
		},
	};

	sqlx::query!(
		"UPDATE names SET address = $1, profile_picture_url = $2, minimized_profile_picture_url = $3 WHERE username = $4",
		payload.address.to_checksum(None),
		payload
			.profile_picture_url
			.as_ref()
			.map(ToString::to_string),
		payload
			.minimized_profile_picture_url
			.as_ref()
			.map(ToString::to_string),
		username
	)
	.execute(&db.read_write)
	.instrument(info_span!("update_record_update", username = username))
	.await?;

	Ok(StatusCode::OK)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Update the details attached to a World App username.")
}
