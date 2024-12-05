use aide::openapi::{self, OpenApi};
use anyhow::Result;
use axum::Extension;
use std::{env, net::SocketAddr};
use tokio::{net::TcpListener, signal};
use tower_http::trace::TraceLayer;

use crate::{config::Config, routes};

#[tracing::instrument(skip_all)]
pub async fn start(mut config: Config) -> Result<()> {
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
        .layer(config.db_extension())
        .layer(config.blocklist_extension())
        .layer(config.extension())
        // Add tracing middleware
        .layer(TraceLayer::new_for_http());

	let addr = SocketAddr::from((
		[0, 0, 0, 0],
		env::var("PORT").map_or(Ok(8000), |p| p.parse())?,
	));
	let listener = TcpListener::bind(&addr).await?;

	tracing::info!("Starting server on {addr}...");

	axum::serve(listener, router.into_make_service())
		.with_graceful_shutdown(shutdown_signal())
		.await?;

	Ok(())
}

#[tracing::instrument]
async fn shutdown_signal() {
	let ctrl_c = async {
		signal::ctrl_c()
			.await
			.expect("failed to install Ctrl+C handler");
	};

	#[cfg(unix)]
	let terminate = async {
		signal::unix::signal(signal::unix::SignalKind::terminate())
			.expect("failed to install signal handler")
			.recv()
			.await;
	};

	#[cfg(not(unix))]
	let terminate = std::future::pending::<()>();

	tokio::select! {
		() = ctrl_c => {},
		() = terminate => {},
	}
}
