use std::str::FromStr;

use alloy::primitives::Address;
use axum::{
	extract::Path,
	response::{IntoResponse, Redirect, Response},
	Extension,
};
use axum_jsonschema::Json;

use crate::{
	config::Db,
	types::{ErrorResponse, MovedRecord, Name, UsernameRecord},
};

#[tracing::instrument(skip(db))]
pub async fn query_single(
	Extension(db): Extension<Db>,
	Path(name_or_address): Path<String>,
) -> Result<Response, ErrorResponse> {
	let query_names_span = tracing::span!(tracing::Level::INFO, "query_names_table", query_type = "SELECT");
	let _names_enter = query_names_span.enter();
	if let Some(name) = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE username = $1 OR address = $1",
		validate_address(&name_or_address)
	)
	.fetch_optional(&db.read_only)
	.await?
	{
		return Ok(Json(UsernameRecord::from(name)).into_response());
	};
	drop(_names_enter);

	let query_moved_span = tracing::span!(tracing::Level::INFO, "query_old_names_table", query_type = "SELECT");
	let _moved_enter = query_moved_span.enter();
	if let Some(moved) = sqlx::query_as!(
		MovedRecord,
		"SELECT * FROM old_names WHERE old_username = $1",
		name_or_address
	)
	.fetch_optional(&db.read_only)
	.await?
	{
		return Ok(Redirect::permanent(&format!("/api/v1/{}", moved.new_username)).into_response());
	}
	drop(_moved_enter);

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
