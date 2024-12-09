use aide::openapi::{self, OpenApi};
use anyhow::Result;
use axum::Extension;
use std::{env, net::SocketAddr};
use tokio::{net::TcpListener, signal};
use tower_http::trace::TraceLayer;
use uuid::Uuid;

use crate::{config::Config, routes};

pub async fn start(mut config: Config) -> Result<()> {
	// Create OpenAPI info
	let mut openapi = OpenApi {
		info: openapi::Info {
			title: "World App Username API".to_string(),
			version: env!("CARGO_PKG_VERSION").to_string(),
			..openapi::Info::default()
		},
		..OpenApi::default()
	};

	// Define the router with the necessary layers
	let router = routes::handler()
		.finish_api(&mut openapi)
		.layer(Extension(openapi))
		.layer(config.db_extension())
		.layer(config.blocklist_extension())
		.layer(config.extension())
		.layer(
			TraceLayer::new_for_http()
				.on_request(|request: &http::Request<_>, _span: &tracing::Span| {
					tracing::info!(method = %request.method(), uri = %request.uri(), "received request");
				})
				.on_response(
					|response: &http::Response<_>,
					 latency: std::time::Duration,
					 _span: &tracing::Span| {
						tracing::info!(status = response.status().as_u16(), latency = ?latency, "response sent");
					},
				)
				.make_span_with(|request: &http::Request<_>| {
					let trace_id = Uuid::new_v4().to_string();

					// println!("	: {}", trace_id);

					tracing::info!("Extracted trace_id: {}", trace_id);

					tracing::info_span!(
						"http_request",
						method = %request.method(),
						uri = %request.uri(),
						trace_id = %trace_id
					)
				}),
		);

	// Server setup and binding
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
