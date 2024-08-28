use aide::axum::{
    routing::{get, post},
    ApiRouter,
};

mod ens_gateway;
mod query_multiple;
mod query_single;
mod register_username;

use ens_gateway::ens_gateway;
use query_multiple::query_multiple;
use query_single::query_single;
use register_username::register_username;

pub fn handler() -> ApiRouter {
    ApiRouter::new()
        .api_route("/ens", post(ens_gateway))
        .api_route("/query", post(query_multiple))
        .api_route("/register", post(register_username))
        .api_route("/:name_or_address", get(query_single))
}
