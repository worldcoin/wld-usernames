use aide::openapi::{self, OpenApi};
use anyhow::Result;
use axum::Extension;
use sqlx::PgPool;
use std::{env, net::SocketAddr};
use tokio::{net::TcpListener, signal};

use crate::{blocklist::Blocklist, routes};

pub async fn start(postgres: PgPool, blocklist: Blocklist) -> Result<()> {
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
        .layer(Extension(postgres))
        .layer(blocklist.extension());

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
