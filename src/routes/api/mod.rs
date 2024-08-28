use aide::axum::ApiRouter;

mod v1;

pub fn handler() -> ApiRouter {
    ApiRouter::new().nest("/v1", v1::handler())
}
