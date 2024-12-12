#![warn(clippy::all, clippy::pedantic, clippy::nursery)]

mod blocklist;
mod config;
mod routes;
mod server;
mod types;
mod utils;
mod verify;

use telemetry_batteries::tracing::{
	datadog::DatadogBattery, stdout::StdoutBattery, TracingShutdownHandle,
};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
	dotenvy::dotenv().ok();

	let _tracing_shutdown_handle = init_telemetry();

	tracing::info!("👩 Server started");

	let config = config::Config::from_env().await?;
	config.migrate_database().await?;

	tracing::info!("👩‍🌾 Migrations run");

	server::start(config).await
}

fn init_telemetry() -> TracingShutdownHandle {
	let traces_endpoint = Some("http://localhost:8126".to_string());

	traces_endpoint
		.as_ref()
		.map_or_else(StdoutBattery::init, |traces_endpoint| {
			let handle = DatadogBattery::init(Some(traces_endpoint), "wld-usernames", None, true);

			handle
		})
}
