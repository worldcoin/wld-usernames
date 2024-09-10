use aide::axum::{routing::get_with, ApiRouter};

mod api;
mod docs;
mod health;
mod system;

use health::{docs as health_docs, health};

pub fn handler() -> ApiRouter {
	ApiRouter::new()
		.merge(docs::handler())
		.merge(system::handler())
		.api_route("/health", get_with(health, health_docs))
		.nest("/api", api::handler())
}
