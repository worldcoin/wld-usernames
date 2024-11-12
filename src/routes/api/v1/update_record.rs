use axum::{extract::Path, Extension};
use axum_jsonschema::Json;
use http::StatusCode;
use sqlx::PgPool;

use crate::{
	config::ConfigExt,
	types::{ErrorResponse, Name, UpdateUsernamePayload},
	verify,
};

#[allow(dependency_on_unit_never_type_fallback)]
pub async fn update_record(
	Path(username): Path<String>,
	Extension(config): ConfigExt,
	Extension(db): Extension<PgPool>,
	Json(payload): Json<UpdateUsernamePayload>,
) -> Result<StatusCode, ErrorResponse> {
	let Some(record) = sqlx::query_as!(Name, "SELECT * FROM names WHERE username = $1", username)
		.fetch_optional(&db)
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
				"Update Record Verification Error: {}",
				payload.verification_level
			);
			return Err(ErrorResponse::validation_error(e.detail));
		},
		Err(_) => {
			return Err(ErrorResponse::server_error(
				"Failed to verify World ID proof".to_string(),
			))
		},
	};

	sqlx::query!(
		"UPDATE names SET address = $1, profile_picture_url = $2 WHERE username = $3",
		payload.address.to_checksum(None),
		payload
			.profile_picture_url
			.as_ref()
			.map(ToString::to_string),
		username
	)
	.execute(&db)
	.await?;

	Ok(StatusCode::OK)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Update the details attached to a World App username.")
}
