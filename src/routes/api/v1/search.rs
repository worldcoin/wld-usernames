use crate::config::DebugClusterClient;
use crate::utils::ONE_MINUTE_IN_SECONDS;
use crate::{
	config::{Db, USERNAME_SEARCH_REGEX},
	types::{ErrorResponse, NameSearch, UsernameRecord},
};
use axum::{
	extract::Path,
	response::{IntoResponse, Response},
	Extension,
};
use axum_jsonschema::Json;
use redis::{Commands, RedisResult};

pub async fn search(
	Extension(db): Extension<Db>,
	Extension(redis): Extension<DebugClusterClient>,
	Path(username): Path<String>,
) -> Result<Response, ErrorResponse> {
	let lowercase_username = username.to_lowercase();

	if !USERNAME_SEARCH_REGEX.is_match(&lowercase_username) {
		return Ok(Json(Vec::<UsernameRecord>::new()).into_response());
	}

	let mut conn = redis.client.get_connection().unwrap();
	let cache_key = format!("search:{lowercase_username}");

	let cached_data = conn.get::<_, String>(&cache_key).unwrap();
	if let Ok(records) = serde_json::from_str::<Vec<UsernameRecord>>(&cached_data) {
		return Ok(Json(records).into_response());
	}

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
	.await?;

	let records: Vec<UsernameRecord> = names.into_iter().map(UsernameRecord::from).collect();

	if let Ok(json_data) = serde_json::to_string(&records) {
		let _: RedisResult<()> = conn.set_ex(&cache_key, json_data, ONE_MINUTE_IN_SECONDS * 5);
	}

	Ok(Json(records).into_response())
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Search for up to 10 usernames. Accepts 1 to 14, only valid username characters to search with.")
		.response::<200, Json<Vec<UsernameRecord>>>()
}
