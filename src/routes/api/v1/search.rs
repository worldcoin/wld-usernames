use crate::types::{ErrorResponse, Name, UsernameRecord};
use axum::{
	extract::Path,
	response::{IntoResponse, Response},
	Extension,
};
use axum_jsonschema::Json;
// use serde::Deserialize;
use sqlx::PgPool;

// #[derive(Deserialize)]
// struct SearchParams {
// 	username: String,
// }

pub async fn search(
	Extension(db): Extension<PgPool>,
	Path(username): Path<String>,
	// Query(params): Query<SearchParams>,
) -> Result<Response, ErrorResponse> {
	let names = sqlx::query_as!(
		Name,
		"SELECT * FROM names
			WHERE username % $1
			ORDER BY similarity(username, $1) DESC
			LIMIT 10;",
		username
	)
	.fetch_all(&db)
	.await?;

	if names.is_empty() {
		return Err(ErrorResponse::not_found("No usernames found.".to_string()));
	}

	return Ok(Json(
		names
			.into_iter()
			.map(UsernameRecord::from)
			.collect::<Vec<UsernameRecord>>(),
	)
	.into_response());
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Search for up to 10 usernames")
		.response::<404, ErrorResponse>()
		.response::<200, Json<Vec<UsernameRecord>>>()
}
