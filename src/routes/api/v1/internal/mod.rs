use aide::axum::{routing::delete_with, ApiRouter};

mod delete_username;

use delete_username::{delete_username, docs as delete_username_docs};

pub fn handler() -> ApiRouter {
	ApiRouter::new().api_route(
		"/usernames/:address",
		delete_with(delete_username, delete_username_docs),
	)
}
