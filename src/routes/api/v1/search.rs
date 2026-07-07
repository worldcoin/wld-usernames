use std::time::Duration;

use anyhow::Context;

use crate::{
	config::{get_or_init_opensearch_client, ConfigExt, USERNAME_SEARCH_REGEX},
	types::{ErrorResponse, UsernameRecord},
	utils::ONE_MINUTE_IN_SECONDS,
};
use axum::{
	extract::Path,
	response::{IntoResponse, Response},
	Extension,
};
use axum_jsonschema::Json;
use redis::{aio::ConnectionManager, AsyncCommands};
use tokio::time::timeout;
use tracing::{info_span, Instrument};

pub async fn search(
	Extension(config): ConfigExt,
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

	// OpenSearch is a hard dependency for this endpoint, but a bounded one. The
	// whole operation — lazily (re)initialising the client if a transient
	// startup failure left it unset, then querying — runs under a single hard
	// deadline, so a slow or unavailable OpenSearch never hangs the request. On
	// any failure we return a retryable 503, rather than a 500, a hang, or (as
	// before) a process-aborting panic when the client is missing.
	let deadline = config.search_opensearch_timeout;
	// Ask OpenSearch to self-limit slightly before our hard client deadline so
	// we usually get a clean response back rather than dropping the connection.
	let server_timeout = deadline
		.saturating_sub(Duration::from_millis(250))
		.max(Duration::from_millis(100));

	let query = async {
		let client = get_or_init_opensearch_client()
			.await
			.context("OpenSearch client unavailable")?;
		client
			.search_usernames(&lowercase_username, 10, server_timeout)
			.instrument(info_span!(
				"search_opensearch_query",
				username = lowercase_username
			))
			.await
	};

	let records = match timeout(deadline, query).await {
		Ok(Ok(records)) => records,
		Ok(Err(e)) => {
			tracing::error!(error = %e, "OpenSearch search failed");
			return Err(ErrorResponse::service_unavailable(
				"Search is temporarily unavailable".to_string(),
			));
		},
		Err(_elapsed) => {
			tracing::error!(timeout = ?deadline, "OpenSearch search timed out");
			return Err(ErrorResponse::service_unavailable(
				"Search is temporarily unavailable".to_string(),
			));
		},
	};

	// cache the results
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
