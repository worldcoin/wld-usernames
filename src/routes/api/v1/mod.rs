use aide::axum::{
	routing::{delete_with, get_with, post_with},
	ApiRouter,
};
use axum::{middleware, routing::post as axum_post, Extension};
use std::sync::Arc;

mod avatar;
mod delete_profile_picture;
mod ens_gateway;
mod profile_picture;
mod query_multiple;
mod query_single;
mod register_username;
mod rename;
mod search;
mod update_record;

use avatar::{avatar, docs as avatar_docs};
use delete_profile_picture::{delete_profile_picture, docs as delete_profile_picture_docs};
use ens_gateway::{docs as ens_gateway_docs, ens_gateway_get, ens_gateway_post};
use http::Method;
use profile_picture::upload_profile_picture;
use query_multiple::{docs as query_multiple_docs, query_multiple};
use query_single::{docs as query_single_docs, query_single, validate_address};
use register_username::{docs as register_username_docs, register_username};
use rename::{docs as rename_docs, rename};
use search::{docs as search_docs, search};
use tower_http::cors::{Any, CorsLayer};
use update_record::{docs as update_record_docs, update_record};

use crate::attestation::{attestation_middleware, JwksCache};

pub fn handler() -> ApiRouter {
	let cors = CorsLayer::new()
	.allow_origin(Any) // Or you can specify allowed origins
	.allow_methods(vec![Method::GET, Method::POST, Method::OPTIONS, Method::DELETE]) // Allow OPTIONS method
	.allow_headers(Any); // Allow any headers

	ApiRouter::new()
		.api_route("/ens", post_with(ens_gateway_post, ens_gateway_docs))
		.layer(cors.clone())
		.api_route("/ens/", post_with(ens_gateway_post, ens_gateway_docs))
		.layer(cors.clone())
		.api_route(
			"/ens/:sender/:data",
			get_with(ens_gateway_get, ens_gateway_docs),
		)
		.layer(cors.clone())
		.api_route("/query", post_with(query_multiple, query_multiple_docs))
		.layer(cors.clone())
		.api_route("/avatar/:name", get_with(avatar, avatar_docs))
		.layer(cors.clone())
		.api_route("/rename", post_with(rename, rename_docs))
		.api_route(
			"/register",
			post_with(register_username, register_username_docs),
		)
		.api_route(
			"/:name",
			get_with(query_single, query_single_docs)
				.post_with(update_record, update_record_docs)
				.layer(cors.clone()),
		)
		.api_route(
			"/search/:username",
			get_with(search, search_docs).layer(cors.clone()),
		)
		.api_route(
			"/profile-picture",
			delete_with(delete_profile_picture, delete_profile_picture_docs).layer(cors.clone()),
		)
		.route(
			"/profile-picture",
			axum_post(upload_profile_picture).route_layer(middleware::from_fn(
				|Extension(jwks_cache): Extension<Arc<JwksCache>>, headers, request, next| async move {
					attestation_middleware(jwks_cache, headers, request, next).await
				},
			)),
		)
}
