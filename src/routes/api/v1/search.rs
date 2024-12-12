use crate::{
	config::{Db, USERNAME_SEARCH_REGEX},
	types::{ErrorResponse, Name, UsernameRecord},
};
use axum::{
	extract::Path,
	response::{IntoResponse, Response},
	Extension,
};
use axum_jsonschema::Json;

#[tracing::instrument(skip_all)]
pub async fn search(
	Extension(db): Extension<Db>,
	Path(username): Path<String>,
) -> Result<Response, ErrorResponse> {
	let lowercase_username = username.to_lowercase();

	if !USERNAME_SEARCH_REGEX.is_match(&lowercase_username) {
		return Ok(Json(Vec::<UsernameRecord>::new()).into_response());
	}

	let names = sqlx::query_as!(
		Name,
		"SELECT * FROM names
			WHERE username % $1
			ORDER BY similarity(username, $1) DESC
			LIMIT 10;",
		lowercase_username
	)
	.fetch_all(&db.read_only)
	.await?;

	return Ok(Json(
		names
			.into_iter()
			.map(UsernameRecord::from)
			.collect::<Vec<UsernameRecord>>(),
	)
	.into_response());
}

#[tracing::instrument(skip_all)]
pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Search for up to 10 usernames. Accepts 1 to 14, only valid username characters to search with.")
		.response::<200, Json<Vec<UsernameRecord>>>()
}
