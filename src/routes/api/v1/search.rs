use crate::types::{ErrorResponse, Name, UsernameRecord};
use axum::{
	extract::Path,
	response::{IntoResponse, Response},
	Extension,
};
use axum_jsonschema::Json;

use sqlx::PgPool;

pub async fn search(
	Extension(db): Extension<PgPool>,
	Path(username): Path<String>,
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
		.response::<200, Json<Vec<UsernameRecord>>>()
}
