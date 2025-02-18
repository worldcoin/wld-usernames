use axum::{
	extract::{Path, Query},
	response::{IntoResponse, Redirect, Response},
	Extension,
};
use redis::{aio::ConnectionManager, AsyncCommands};
use tracing::{info_span, Instrument};

use crate::{
	config::Db,
	types::{AvatarQueryParams, ErrorResponse, MovedRecord},
	utils::ONE_MINUTE_IN_SECONDS,
};

#[tracing::instrument(skip_all)]
pub async fn avatar(
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Query(params): Query<AvatarQueryParams>,
	Path(name): Path<String>,
) -> Result<Response, ErrorResponse> {
	let cache_key = format!("avatar:{name}");

	if let Ok(avatar_url) = redis.get::<_, String>(&cache_key).await {
		return Ok(Redirect::temporary(&avatar_url).into_response());
	}

	if let Some(record) = sqlx::query!(
		"SELECT username, profile_picture_url FROM names WHERE LOWER(username) = LOWER($1)",
		name
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!("avatar_db_query", input = name))
	.await?
	{
		if let Some(profile_picture_url) = record.profile_picture_url {
			redis
				.set_ex(
					&cache_key,
					&profile_picture_url,
					ONE_MINUTE_IN_SECONDS * 60 * 24 * 7,
				)
				.await?;

			return Ok(Redirect::temporary(&profile_picture_url).into_response());
		}
	}

	if let Some(moved) = sqlx::query_as!(
		MovedRecord,
		"SELECT * FROM old_names WHERE old_username = $1",
		name
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!("avatar_moved_db_query", username = name))
	.await?
	{
		return Ok(
			Redirect::permanent(&format!("/api/v1/avatar/{}", moved.new_username)).into_response(),
		);
	}

	if let Some(fallback) = params.fallback {
		return Ok(Redirect::temporary(fallback.as_str()).into_response());
	}

	Err(ErrorResponse::not_found("Record not found.".to_string()))
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Redirect to the user's avatar, optionally falling back to a default.")
		.response_with::<404, ErrorResponse, _>(|op| {
			op.description(
				"Returned when the user has no avatar and a fallback image is not provided.",
			)
		})
		.response_with::<301, Redirect, _>(|op| {
			op.description("A redirect to the user's avatar or the fallback avatar.")
		})
}
