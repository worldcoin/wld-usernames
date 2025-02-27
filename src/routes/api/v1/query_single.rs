use std::str::FromStr;

use alloy::primitives::Address;
use axum::{
	extract::Path,
	response::{IntoResponse, Redirect, Response},
	Extension,
};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands};
use tracing::{info_span, Instrument};

use crate::{
	config::Db,
	types::{ErrorResponse, MovedRecord, Name, UsernameRecord},
	utils::ONE_MINUTE_IN_SECONDS,
};

#[tracing::instrument(skip_all)]
pub async fn query_single(
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Path(name_or_address): Path<String>,
) -> Result<Response, ErrorResponse> {
	let validated_input = validate_address(&name_or_address);

	let cache_key = format!("query_single:{validated_input}");

	if let Ok(cached_data) = redis.get::<_, String>(&cache_key).await {
		if let Ok(record) = serde_json::from_str::<UsernameRecord>(&cached_data) {
			return Ok(Json(record).into_response());
		}
	}

	if let Some(name) = sqlx::query_as!(
		Name,
		r#"
        SELECT 
            username as "username!",
            address as "address!",
            profile_picture_url,
            nullifier_hash as "nullifier_hash!",
            verification_level as "verification_level!",
            created_at as "created_at!",
            updated_at as "updated_at!"
        FROM names 
        WHERE LOWER(username) = LOWER($1) 
        UNION ALL 
        SELECT 
            username as "username!",
            address as "address!",
            profile_picture_url,
            nullifier_hash as "nullifier_hash!",
            verification_level as "verification_level!",
            created_at as "created_at!",
            updated_at as "updated_at!"
        FROM names 
        WHERE address = $1 AND LOWER(username) <> LOWER($1)
        "#,
		validated_input
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!("query_single_db_query", input = validated_input))
	.await?
	{
		let record = UsernameRecord::from(name);
		// long cache because we can effectively invalidate
		if let Ok(json_data) = serde_json::to_string(&record) {
			let _: Result<(), redis::RedisError> = redis
				.set_ex(&cache_key, json_data, ONE_MINUTE_IN_SECONDS * 60 * 24 * 7)
				.await;
		}
		return Ok(Json(record).into_response());
	}

	if let Some(moved) = sqlx::query_as!(
		MovedRecord,
		"SELECT * FROM old_names WHERE old_username = $1",
		name_or_address
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!(
		"query_single_moved_db_query",
		username = name_or_address
	))
	.await?
	{
		return Ok(Redirect::permanent(&format!("/api/v1/{}", moved.new_username)).into_response());
	}

	Err(ErrorResponse::not_found("Record not found.".to_string()))
}

/// A version of query_single that always includes the updated_at timestamp and doesn't cache
#[tracing::instrument(skip_all)]
pub async fn query_single_with_timestamp(
	Extension(db): Extension<Db>,
	Path(name_or_address): Path<String>,
) -> Result<Response, ErrorResponse> {
	let validated_input = validate_address(&name_or_address);

	if let Some(name) = sqlx::query_as!(
		Name,
		r#"
        SELECT 
            username as "username!",
            address as "address!",
            profile_picture_url,
            nullifier_hash as "nullifier_hash!",
            verification_level as "verification_level!",
            created_at as "created_at!",
            updated_at as "updated_at!"
        FROM names 
        WHERE LOWER(username) = LOWER($1) 
        UNION ALL 
        SELECT 
            username as "username!",
            address as "address!",
            profile_picture_url,
            nullifier_hash as "nullifier_hash!",
            verification_level as "verification_level!",
            created_at as "created_at!",
            updated_at as "updated_at!"
        FROM names 
        WHERE address = $1 AND LOWER(username) <> LOWER($1)
        "#,
		validated_input
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!(
		"query_single_timestamp_db_query",
		input = validated_input
	))
	.await?
	{
		let updated_at = name.updated_at.clone();
		let mut record = UsernameRecord::from(name);
		record.updated_at = Some(updated_at);
		return Ok(Json(record).into_response());
	}

	if let Some(moved) = sqlx::query_as!(
		MovedRecord,
		"SELECT * FROM old_names WHERE old_username = $1",
		name_or_address
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!(
		"query_single_timestamp_moved_db_query",
		username = name_or_address
	))
	.await?
	{
		return Ok(
			Redirect::permanent(&format!("/api/v1/timestamp/{}", moved.new_username))
				.into_response(),
		);
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

pub fn timestamp_docs(
	op: aide::transform::TransformOperation,
) -> aide::transform::TransformOperation {
	op.description("Resolve a single username or address with timestamp information.")
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
