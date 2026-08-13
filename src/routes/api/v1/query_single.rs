use std::str::FromStr;

use alloy::primitives::Address;
use axum::{
	extract::Path,
	response::{IntoResponse, Redirect, Response},
	Extension,
};
use axum_jsonschema::Json;
use redis::aio::ConnectionManager;
use tracing::{info_span, Instrument};

use crate::{
	cache,
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

	let cache_key = format!(
		"query_single:{}",
		cache::canonical_cache_input(&name_or_address)
	);

	let mut deletion_tombstoned = false;
	match cache::lookup(&mut redis, &cache_key).await {
		cache::Lookup::Hit(cached_data) => {
			if let Ok(record) = serde_json::from_str::<UsernameRecord>(&cached_data) {
				return Ok(Json(record).into_response());
			}
		},
		cache::Lookup::Tombstoned => deletion_tombstoned = true,
		cache::Lookup::Miss => {},
	}

	// A tombstoned key means the record was just deleted: a read replica may
	// still return the deleted row, so verify against the primary instead.
	// (A row found there is a legitimate re-registration of the freed name.)
	let db_pool = if deletion_tombstoned {
		&db.read_write
	} else {
		&db.read_only
	};

	if let Some(name) = sqlx::query_as!(
		Name,
		r#"
        SELECT 
            username as "username!",
            address as "address!",
            profile_picture_url,
            minimized_profile_picture_url,
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
            minimized_profile_picture_url,
            nullifier_hash as "nullifier_hash!",
            verification_level as "verification_level!",
            created_at as "created_at!",
            updated_at as "updated_at!"
        FROM names 
        WHERE address = $1 AND LOWER(username) <> LOWER($1)
        "#,
		validated_input
	)
	.fetch_optional(db_pool)
	.instrument(info_span!("query_single_db_query", input = validated_input))
	.await?
	{
		let record = UsernameRecord::from(name);
		// long cache because we can effectively invalidate
		if let Ok(json_data) = serde_json::to_string(&record) {
			let _ = cache::set_ex_unless_tombstoned(
				&mut redis,
				&cache_key,
				&json_data,
				ONE_MINUTE_IN_SECONDS * 60 * 24,
			)
			.await;
		}
		return Ok(Json(record).into_response());
	}

	if let Some(moved) = sqlx::query_as!(
		MovedRecord,
		"SELECT * FROM old_names WHERE old_username = $1",
		name_or_address
	)
	.fetch_optional(db_pool)
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
