use aide::axum::ApiRouter;

mod v1;
pub mod v2;

pub fn handler() -> ApiRouter {
	ApiRouter::new()
		.nest("/v1", v1::handler())
		.nest("/v2", v2::handler())
}
