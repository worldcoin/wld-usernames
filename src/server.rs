use aide::openapi::{self, OpenApi};
use anyhow::Result;
use axum::{body::Body, http::Request, middleware::Next, response::Response, Extension};
use http::Method;
use http_body_util::{BodyExt, Collected};
use std::{env, net::SocketAddr, time::Duration};
use tokio::{net::TcpListener, signal};
use tower_http::{
	compression::CompressionLayer,
	timeout::TimeoutLayer,
	trace::{DefaultMakeSpan, TraceLayer},
};
use tracing::Span;

use crate::{config::Config, routes};

#[must_use]
pub fn get_timeout_layer(timeout: Option<u64>) -> TimeoutLayer {
	let timeout = timeout.map_or(Duration::from_secs(20), Duration::from_secs);
	TimeoutLayer::new(timeout)
}

/// Adds the request method to the response extensions so that it can be used in the trace layer.
async fn record_request_method(req: axum::extract::Request, next: Next) -> Response {
	let method = req.method().clone();
	let path = req.uri().path().to_string();
	let mut response = next.run(req).await;
	response.extensions_mut().insert((method, path));
	response
}

async fn record_bad_request(req: axum::extract::Request, next: Next) -> Response {
	let path = req.uri().path().to_string();
	let response = next.run(req).await;

	let (response_parts, response_body) = response.into_parts();
	let bytes = response_body
		.collect()
		.await
		.unwrap_or_else(|_| Collected::default())
		.to_bytes();
	let status = response_parts.status.as_u16();
	if status != 200 && status != 404 {
		let body_str = std::str::from_utf8(&bytes).unwrap_or_default();
		tracing::debug!(
			http.route = ?path,
			http.status_code = status,
			response_body = body_str,
			"👾 returning non-200 response"
		);
	}
	Response::from_parts(response_parts, Body::from(bytes))
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
		.layer(CompressionLayer::new())
		.layer(axum::middleware::from_fn(record_bad_request))
		.layer(axum::middleware::from_fn(record_request_method))
		.layer(
			TraceLayer::new_for_http()
				.make_span_with(DefaultMakeSpan::new().include_headers(true))
				.on_request(|request: &Request<_>, _span: &Span| {
					if request.method() != Method::GET {
						tracing::debug!(
							content_length_header = request.headers().get("content-length").map(|v| v.to_str().unwrap_or_default()),
							path = request.uri().path(),
							"📥 received request: {} {}",
							request.method(),
							request.uri().path()
						);
					}
				})
				.on_response(|response: &Response, latency: Duration, _span: &Span| {
					let ext = response.extensions().get::<(Method, String)>();
					let status = response.status().as_u16();

					if let Some((method, path)) = ext {
						if method != Method::GET {
							tracing::debug!(
								http.route = ?path,
								http.status_code = status,
								http.method = method.to_string(),
								latency = ?latency,
								response_headers = ?response.headers(),
								"🔚 finished processing {} request in {} ms ({})",
								method,
								latency.as_millis(),
								status,
							);
						}
					}
				}),
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
