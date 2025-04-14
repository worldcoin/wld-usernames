use crate::{
	config::{get_opensearch_client, Db, USERNAME_SEARCH_REGEX},
	types::{ErrorResponse, NameSearch, UsernameRecord},
	utils::ONE_MINUTE_IN_SECONDS,
};
use axum::{
	extract::Path,
	response::{IntoResponse, Response},
	Extension,
};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands};
use tracing::{info, info_span, Instrument};

pub async fn search(
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Path(username): Path<String>,
) -> Result<Response, ErrorResponse> {
	let lowercase_username = username.to_lowercase();

	if !USERNAME_SEARCH_REGEX.is_match(&lowercase_username) {
		return Ok(Json(Vec::<UsernameRecord>::new()).into_response());
	}

	let cache_key = format!("search:{lowercase_username}");

	// try to get results from cache first
	if let Ok(cached_data) = redis.get::<_, String>(&cache_key).await {
		if let Ok(records) = serde_json::from_str::<Vec<UsernameRecord>>(&cached_data) {
			return Ok(Json(records).into_response());
		}
	}

	// try to use OpenSearch if available
	if let Some(opensearch_client) = get_opensearch_client() {
		info!("Using OpenSearch for search query: {}", lowercase_username);

		match opensearch_client
			.search_usernames(&lowercase_username, 10)
			.instrument(info_span!(
				"search_opensearch_query",
				username = lowercase_username
			))
			.await
		{
			Ok(records) => {
				tracing::info!("OpenSearch search successful: {:?}", records);

				// Fall back to PostgreSQL if OpenSearch returns empty results
				if records.is_empty() {
					tracing::info!("OpenSearch returned no results, falling back to PostgreSQL");
				} else {
					// cache the results
					if let Ok(json_data) = serde_json::to_string(&records) {
						let _: Result<(), redis::RedisError> = redis
							.set_ex(&cache_key, json_data, ONE_MINUTE_IN_SECONDS * 5)
							.await;
					}

					return Ok(Json(records).into_response());
				}
			},
			Err(e) => {
				// log the error but fall back to PostgreSQL
				tracing::error!("OpenSearch search failed: {}", e);
			},
		}
	}

	// fall back to PostgreSQL if OpenSearch is not available or fails
	let names = sqlx::query_as!(
		NameSearch,
		"SELECT username,
			address,
			profile_picture_url
			FROM names
			WHERE username % $1
			ORDER BY username <-> $1
			LIMIT 10;",
		lowercase_username
	)
	.fetch_all(&db.read_only)
	.instrument(info_span!("search_db_query", username = lowercase_username))
	.await?;

	let records: Vec<UsernameRecord> = names.into_iter().map(UsernameRecord::from).collect();

	if let Ok(json_data) = serde_json::to_string(&records) {
		let _: Result<(), redis::RedisError> = redis
			.set_ex(&cache_key, json_data, ONE_MINUTE_IN_SECONDS * 5)
			.await;
	}

	Ok(Json(records).into_response())
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Search for up to 10 usernames. Accepts 1 to 14, only valid username characters to search with.")
		.response::<200, Json<Vec<UsernameRecord>>>()
}
