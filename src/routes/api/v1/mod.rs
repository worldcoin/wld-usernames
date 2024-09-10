use aide::axum::{
	routing::{get_with, post_with},
	ApiRouter,
};

mod ens_gateway;
mod health;
mod query_multiple;
mod query_single;
mod register_username;
mod rename;
mod update_record;

use ens_gateway::{docs as ens_gateway_docs, ens_gateway};
use health::{docs as health_docs, health};
use query_multiple::{docs as query_multiple_docs, query_multiple};
use query_single::{docs as query_single_docs, query_single};
use register_username::{docs as register_username_docs, register_username};
use rename::{docs as rename_docs, rename};
use update_record::{docs as update_record_docs, update_record};

pub fn handler() -> ApiRouter {
	ApiRouter::new()
		.api_route("/ens", post_with(ens_gateway, ens_gateway_docs))
		.api_route("/query", post_with(query_multiple, query_multiple_docs))
		.api_route("/rename", post_with(rename, rename_docs))
		.api_route(
			"/register",
			post_with(register_username, register_username_docs),
		)
		.api_route(
			"/:name",
			get_with(query_single, query_single_docs).post_with(update_record, update_record_docs),
		)
		.api_route("/health", get_with(health, health_docs))
}
