use std::str::FromStr;

use alloy::primitives::Address;
use axum::{
	extract::Path,
	response::{IntoResponse, Redirect, Response},
	Extension,
};
use axum_jsonschema::Json;
use sqlx::PgPool;

use crate::types::{ErrorResponse, MovedRecord, Name, UsernameRecord};

pub async fn query_single(
	Extension(db): Extension<PgPool>,
	Path(name_or_address): Path<String>,
) -> Result<Response, ErrorResponse> {
	if let Some(name) = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE username = $1 OR address = $1",
		validate_address(&name_or_address)
	)
	.fetch_optional(&db)
	.await?
	{
		return Ok(Json(UsernameRecord::from(name)).into_response());
	};

	if let Some(moved) = sqlx::query_as!(
		MovedRecord,
		"SELECT * FROM old_names WHERE old_username = $1",
		name_or_address
	)
	.fetch_optional(&db)
	.await?
	{
		return Ok(Redirect::permanent(&format!("/api/v1/{}", moved.new_username)).into_response());
	}

	Err(ErrorResponse::not_found("Record not found.".to_string()))
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Resolve a single username or address.")
		.response::<404, ErrorResponse>()
		.response::<200, Json<UsernameRecord>>()
		.response_with::<301, Redirect, _>(|op| {
			op.description(
				"A redirect to the new username, if the queries username has recently changed.",
			)
		})
}

pub fn validate_address(name_or_address: &str) -> String {
	Address::from_str(name_or_address).map_or_else(
		|_| name_or_address.to_string(),
		|address| address.to_checksum(None),
	)
}
