use anyhow::Context;
use axum::Extension;
use idkit::session::AppId;
use once_cell::sync::OnceCell;
use redis::aio::ConnectionManager;
use regex::Regex;
use sqlx::{migrate::MigrateError, postgres::PgPoolOptions, PgPool};
use std::{
	env::{self, VarError},
	fmt::{self, Debug, Formatter},
	num::ParseIntError,
	sync::{Arc, LazyLock},
	time::Duration,
};

use crate::{
	blocklist::{Blocklist, BlocklistExt},
	search::OpenSearchClient,
};

#[allow(clippy::module_name_repetitions)]
pub type ConfigExt = Extension<Arc<Config>>;

pub static USERNAME_REGEX: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[a-z]\w{2,13}[a-z0-9]$").unwrap());
pub static DEVICE_USERNAME_REGEX: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[a-z]\w{2,13}[a-z0-9]\.\d{4}$").unwrap());
pub static USERNAME_SEARCH_REGEX: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[a-z]\w{0,13}([a-z0-9](\.\d{1,4})?)$").unwrap());

pub static OPENSEARCH_CLIENT: OnceCell<Arc<OpenSearchClient>> = OnceCell::new();

#[derive(Clone)]
pub struct ConnectionManagerDebug {
	pub connection: ConnectionManager,
}
impl Debug for ConnectionManagerDebug {
	fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
		write!(f, "ConnectionManagerDebug",)
	}
}

impl From<ConnectionManager> for ConnectionManagerDebug {
	fn from(connection: ConnectionManager) -> Self {
		Self { connection }
	}
}

#[derive(Debug)]
pub struct Config {
	pub wld_app_id: AppId,
	pub ens_domain: String,
	pub private_key: String,
	pub developer_portal_url: String,
	pub whitelisted_avatar_domains: Option<Vec<String>>,
	db_client: Option<PgPool>,
	db_read_client: Option<PgPool>,
	redis_pool: Option<ConnectionManagerDebug>,
	blocklist: Option<Blocklist>,
}
#[derive(Clone)]
pub struct Db {
	pub read_only: PgPool,
	pub read_write: PgPool,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Env(#[from] VarError),
	#[error(transparent)]
	Sqlx(#[from] sqlx::Error),
	#[error(transparent)]
	ChainId(#[from] ParseIntError),
	#[error(transparent)]
	EnvWithContext(#[from] anyhow::Error),
	#[error(transparent)]
	Redis(#[from] redis::RedisError),
	#[error(transparent)]
	Reqwest(#[from] reqwest::Error),
}

impl Config {
	pub async fn from_env() -> Result<Self, Error> {
		let blocklist = Blocklist::new(
			&env::var("RESERVED_USERNAMES")
				.context("RESERVED_USERNAMES environment variable not set")?,
			&env::var("BLOCKED_SUBSTRINGS")
				.context("BLOCKED_SUBSTRINGS environment variable not set")?,
		);

		let whitelisted_avatar_domains =
			env::var("WHITELISTED_AVATAR_DOMAINS").ok().map(|domains| {
				domains
					.split(',')
					.map(|s| s.trim().to_lowercase())
					.collect()
			});

		let db_client = PgPoolOptions::new()
			.max_connections(50)
			.acquire_timeout(Duration::from_secs(4))
			.connect(
				&env::var("DATABASE_URL").context("DATABASE_URL environment variable not set")?,
			)
			.await?;

		let db_read_client = PgPoolOptions::new()
			.min_connections(10)
			.max_connections(128)
			.acquire_timeout(Duration::from_secs(4))
			.connect(
				&env::var("DATABASE_READ_URL")
					.context("DATABASE_READ_URL environment variable not set")?,
			)
			.await?;

		let redis_url = env::var("REDIS_URL").context("REDIS_URL environment variable not set")?;

		let redis_pool = build_redis_pool(redis_url)
			.await
			.expect("Failed to connect to Redis");

		tracing::info!("✅ Connection to Redis established.");

		// Initialize OpenSearch client
		if OPENSEARCH_CLIENT.get().is_none() {
			match OpenSearchClient::new().await {
				Ok(client) => {
					tracing::info!("✅ Connection to OpenSearch established.");
					let _ = OPENSEARCH_CLIENT.set(Arc::new(client));
				},
				Err(e) => {
					tracing::error!("❌ Failed to connect to OpenSearch: {}", e);
				},
			}
		}

		Ok(Self {
			db_client: Some(db_client),
			db_read_client: Some(db_read_client),
			blocklist: Some(blocklist),
			ens_domain: env::var("ENS_DOMAIN")
				.context("ENS_DOMAIN environment variable not set")?,
			private_key: env::var("PRIVATE_KEY")
				.context("PRIVATE_KEY environment variable not set")?,
			wld_app_id: unsafe {
				AppId::new_unchecked(
					env::var("WLD_APP_ID").context("WLD_APP_ID environment variable not set")?,
				)
			},
			developer_portal_url: env::var("DEVELOPER_PORTAL_ENDPOINT")
				.context("DEVELOPER_PORTAL_ENDPOINT environment variable not set")?,
			redis_pool: Some(ConnectionManagerDebug::from(redis_pool)),
			whitelisted_avatar_domains,
		})
	}

	pub async fn migrate_database(&self) -> Result<(), MigrateError> {
		sqlx::migrate!().run(self.db_client.as_ref().unwrap()).await
	}

	pub fn db_extension(&mut self) -> Extension<Db> {
		Extension(Db {
			read_only: self.db_read_client.take().unwrap(),
			read_write: self.db_client.take().unwrap(),
		})
	}

	pub fn redis_extension(&mut self) -> Extension<ConnectionManager> {
		Extension(self.redis_pool.take().unwrap().connection)
	}

	pub fn get_redis_connection(&self) -> ConnectionManager {
		self.redis_pool.as_ref().unwrap().connection.clone()
	}

	pub fn blocklist_extension(&mut self) -> BlocklistExt {
		Extension(Arc::new(self.blocklist.take().unwrap()))
	}

	pub fn extension(self) -> ConfigExt {
		Extension(Arc::new(self))
	}
}

async fn build_redis_pool(mut redis_url: String) -> redis::RedisResult<ConnectionManager> {
	if !redis_url.starts_with("redis://") && !redis_url.starts_with("rediss://") {
		redis_url = format!("redis://{redis_url}");
	}

	let client = redis::Client::open(redis_url)?;

	ConnectionManager::new(client).await
}

pub fn get_opensearch_client() -> Option<Arc<OpenSearchClient>> {
	OPENSEARCH_CLIENT.get().cloned()
}
