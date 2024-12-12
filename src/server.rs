use aide::openapi::{self, OpenApi};
use anyhow::Result;
use axum::Extension;
use std::{env, net::SocketAddr, time::Duration};
use tokio::{net::TcpListener, signal};
use tower_http::{
	timeout::TimeoutLayer,
	trace::{DefaultMakeSpan, TraceLayer},
};

use crate::{config::Config, routes};

#[must_use]
pub fn get_timeout_layer(timeout: Option<u64>) -> TimeoutLayer {
	let timeout = timeout.map_or(Duration::from_secs(20), Duration::from_secs);
	TimeoutLayer::new(timeout)
}

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
		.layer(
			TraceLayer::new_for_http().make_span_with(DefaultMakeSpan::new().include_headers(true)),
		)
		.layer(get_timeout_layer(None));

	tracing::info!("✅ preflight done. all services initialized...");

	let addr = SocketAddr::from((
		[0, 0, 0, 0],
		env::var("PORT").map_or(Ok(8000), |p| p.parse())?,
	));
	let listener = TcpListener::bind(&addr).await?;

	tracing::info!("🚀 started server on {addr}...");

	axum::serve(listener, router.into_make_service())
		.with_graceful_shutdown(shutdown_signal())
		.await?;

	Ok(())
}

async fn shutdown_signal() {
	let ctrl_c = async {
		signal::ctrl_c()
			.await
			.expect("failed to install Ctrl+C handler");
		tracing::warn!("⚠️ received termination signal...");
	};

	#[cfg(unix)]
	let terminate = async {
		signal::unix::signal(signal::unix::SignalKind::terminate())
			.expect("failed to install signal handler")
			.recv()
			.await;
		tracing::warn!("⚠️ received termination signal...");
	};

	#[cfg(not(unix))]
	let terminate = std::future::pending::<()>();

	tokio::select! {
		() = ctrl_c => {},
		() = terminate => {},
	}
}
