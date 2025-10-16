use aide::openapi::{self, OpenApi};
use anyhow::Result;
use axum::Extension;
use datadog_tracing::axum::{OtelAxumLayer, OtelInResponseLayer};
use std::{env, net::SocketAddr, time::Duration};
use tokio::{net::TcpListener, sync::broadcast};
use tower_http::timeout::TimeoutLayer;

use crate::{config::Config, routes};

pub async fn start(mut config: Config, mut shutdown: broadcast::Receiver<()>) -> Result<()> {
	let mut openapi = OpenApi {
		info: openapi::Info {
			title: "World App Username API".to_string(),
			version: env!("CARGO_PKG_VERSION").to_string(),
			..openapi::Info::default()
		},
		..OpenApi::default()
	};

	// Create JWKS cache extension before config is consumed
	let jwks_cache_ext = config.jwks_cache_extension();

	let router = routes::handler()
		.finish_api(&mut openapi)
		.layer(Extension(openapi))
		.layer(OtelInResponseLayer)
		.layer((
			OtelAxumLayer::default(),
			TimeoutLayer::new(Duration::from_secs(90)),
		))
		.layer(config.db_extension())
		.layer(config.redis_extension())
		.layer(config.blocklist_extension())
		.layer(jwks_cache_ext)
		.layer(config.extension());

	let addr = SocketAddr::from((
		[0, 0, 0, 0],
		env::var("PORT").map_or(Ok(8000), |p| p.parse())?,
	));
	let listener = TcpListener::bind(&addr).await?;

	tracing::info!("Starting server on {addr}...");

	axum::serve(listener, router.into_make_service())
		.with_graceful_shutdown(async move {
			shutdown.recv().await.ok();
		})
		.await?;

	Ok(())
}
