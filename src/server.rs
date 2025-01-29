use aide::openapi::{self, OpenApi};
use anyhow::Result;
use axum::Extension;
use datadog_tracing::axum::{
	shutdown_signal as dd_shutdown_signal, OtelAxumLayer, OtelInResponseLayer,
};
use std::{env, net::SocketAddr, time::Duration};
use tokio::{net::TcpListener, sync::broadcast};
use tower_http::timeout::TimeoutLayer;

use crate::{config::Config, routes};

pub async fn start(mut config: Config, shutdown_tx: broadcast::Sender<()>) -> Result<()> {
	let mut openapi = OpenApi {
		info: openapi::Info {
			title: "World App Username API".to_string(),
			version: env!("CARGO_PKG_VERSION").to_string(),
			..openapi::Info::default()
		},
		..OpenApi::default()
	};

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
		.layer(config.extension());

	let addr = SocketAddr::from((
		[0, 0, 0, 0],
		env::var("PORT").map_or(Ok(8000), |p| p.parse())?,
	));
	let listener = TcpListener::bind(&addr).await?;

	tracing::info!("Starting server on {addr}...");

	// Use the same shutdown signal for both the server and worker
	let server =
		axum::serve(listener, router.into_make_service()).with_graceful_shutdown(async move {
			dd_shutdown_signal().await;
			// When server receives shutdown signal, propagate it to the worker
			let _ = shutdown_tx.send(());
		});

	// Run the server and wait for it to complete
	server.await?;

	Ok(())
}
