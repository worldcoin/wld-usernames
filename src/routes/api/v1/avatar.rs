use axum::{
	extract::{Path, Query},
	response::{IntoResponse, Redirect, Response},
	Extension,
};
use redis::{aio::ConnectionManager, AsyncCommands};
use tracing::{info_span, Instrument};
use url::Url;

use crate::{
	config::{Config, Db},
	types::{AvatarQueryParams, ErrorResponse, MovedRecord},
	utils::ONE_MINUTE_IN_SECONDS,
};

#[tracing::instrument(skip_all)]
pub async fn avatar(
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Extension(config): Extension<Config>,
	Query(params): Query<AvatarQueryParams>,
	Path(name): Path<String>,
) -> Result<Response, ErrorResponse> {
	let minimized = params.minimized.unwrap_or(false);
	let cache_key = format!(
		"avatar:{name}:{}",
		if minimized { "minimized" } else { "original" }
	);

	if let Ok(avatar_url) = redis.get::<_, String>(&cache_key).await {
		return Ok(Redirect::temporary(&avatar_url).into_response());
	}

	if let Some(record) = sqlx::query!(
		"SELECT username, profile_picture_url, minimized_profile_picture_url FROM names WHERE LOWER(username) = LOWER($1)",
		name
	)
	.fetch_optional(&db.read_only)
	.instrument(info_span!("avatar_db_query", input = name))
	.await?
	{
		let profile_picture_url = if minimized {
			record.minimized_profile_picture_url
		} else {
			record.profile_picture_url
		};

		if let Some(profile_picture_url) = profile_picture_url {
			redis
				.set_ex(
					&cache_key,
					&profile_picture_url,
					ONE_MINUTE_IN_SECONDS * 60 * 24,
				)
				.await?;

			return Ok(Redirect::temporary(&profile_picture_url).into_response());
		}

		return Ok(fallback_response(
			params.fallback,
			"Avatar not set".to_string(),
			&config,
		));
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

	Ok(fallback_response(
		params.fallback,
		"Record not found".to_string(),
		&config,
	))
}

fn fallback_response(fallback: Option<Url>, error_msg: String, config: &Config) -> Response {
	fallback.map_or_else(
		|| ErrorResponse::not_found(error_msg).into_response(),
		|fallback| {
			// If whitelist is not set, block all fallback URLs
			if config.whitelisted_avatar_domains.is_none() {
				return ErrorResponse::forbidden(
					"Fallback URLs are not allowed when whitelist is not configured".to_string(),
				)
				.into_response();
			}

			// Check if the fallback URL's domain is in the whitelist
			if let Some(domain) = fallback.host_str() {
				let domain = domain.to_lowercase();
				let whitelist = match config.whitelisted_avatar_domains.as_ref() {
					Some(list) => list,
					None => {
						return ErrorResponse::forbidden(
							"Fallback URLs are not allowed when whitelist is not configured"
								.to_string(),
						)
						.into_response()
					},
				};

				// Check if the domain exactly matches a whitelisted domain
				// or is a subdomain of a whitelisted domain
				if whitelist
					.iter()
					.any(|allowed| domain == *allowed || domain.ends_with(&format!(".{}", allowed)))
				{
					return Redirect::temporary(fallback.as_str()).into_response();
				}
			}

			// If domain is not whitelisted or host is missing, return error
			ErrorResponse::forbidden("Fallback URL domain is not whitelisted".to_string())
				.into_response()
		},
	)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Redirect to the user's avatar, optionally falling back to a default. The fallback URL must be from a whitelisted domain specified in the WHITELISTED_AVATAR_DOMAINS environment variable.")
		.response_with::<404, ErrorResponse, _>(|op| {
			op.description(
				"Returned when the user has no avatar and a fallback image is not provided.",
			)
		})
		.response_with::<403, ErrorResponse, _>(|op| {
			op.description(
				"Returned when the fallback URL's domain is not whitelisted or when fallback URLs are not allowed (whitelist not configured).",
			)
		})
		.response_with::<301, Redirect, _>(|op| {
			op.description("A redirect to the user's avatar or the fallback avatar (if from a whitelisted domain).")
		})
}
