use anyhow::Context;
use axum::Extension;
use idkit::session::AppId;
use redis::{aio::MultiplexedConnection, Client, TlsCertificates};
use regex::Regex;
use sqlx::{migrate::MigrateError, postgres::PgPoolOptions, PgPool};
use std::{
	env::{self, VarError},
	num::ParseIntError,
	sync::{Arc, LazyLock},
	time::Duration,
};

use crate::blocklist::{Blocklist, BlocklistExt};

#[allow(clippy::module_name_repetitions)]
pub type ConfigExt = Extension<Arc<Config>>;

pub static USERNAME_REGEX: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[a-z]\w{2,13}[a-z0-9]$").unwrap());
pub static DEVICE_USERNAME_REGEX: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[a-z]\w{2,13}[a-z0-9]\.\d{4}$").unwrap());
pub static USERNAME_SEARCH_REGEX: LazyLock<Regex> =
	LazyLock::new(|| Regex::new(r"^[a-z]\w{0,13}([a-z0-9](\.\d{1,4})?)$").unwrap());

#[derive(Debug)]
pub struct Config {
	pub wld_app_id: AppId,
	pub ens_domain: String,
	pub private_key: String,
	pub developer_portal_url: String,
	db_client: Option<PgPool>,
	db_read_client: Option<PgPool>,
	redis_connection: Option<MultiplexedConnection>, // Change this field
	blocklist: Option<Blocklist>,
}
#[derive(Clone)]
pub struct Db {
	pub read_only: PgPool,
	pub read_write: PgPool,
}

#[derive(Clone)]
pub struct RedisClient {
	pub connection: MultiplexedConnection,
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
}

impl Config {
	pub async fn from_env() -> Result<Self, Error> {
		let blocklist = Blocklist::new(
			&env::var("RESERVED_USERNAMES")
				.context("RESERVED_USERNAMES environment variable not set")?,
			&env::var("BLOCKED_SUBSTRINGS")
				.context("BLOCKED_SUBSTRINGS environment variable not set")?,
		);

		let db_client = PgPoolOptions::new()
			.max_connections(100)
			.acquire_timeout(Duration::from_secs(3))
			.connect(
				&env::var("DATABASE_URL").context("DATABASE_URL environment variable not set")?,
			)
			.await?;

		let db_read_client = PgPoolOptions::new()
			.acquire_timeout(Duration::from_secs(3))
			.connect(
				&env::var("DATABASE_READ_URL")
					.context("DATABASE_READ_URL environment variable not set")?,
			)
			.await?;

		let redis_url = env::var("REDIS_URL").context("REDIS_URL environment variable not set")?;
		// for local dev
		// let redis_client = Client::open(redis_url)?;
		let redis_client = Client::build_with_tls(
			redis_url,
			TlsCertificates {
				client_tls: None,
				root_cert: None,
			},
		)?;

		let redis_connection = redis_client
			.get_multiplexed_async_connection()
			.await
			.context("Failed to establish Redis connection")?;

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
			redis_connection: Some(redis_connection),
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

	pub fn redis_extension(&mut self) -> Extension<RedisClient> {
		Extension(RedisClient {
			connection: self.redis_connection.take().unwrap(),
		})
	}

	pub fn blocklist_extension(&mut self) -> BlocklistExt {
		Extension(Arc::new(self.blocklist.take().unwrap()))
	}

	pub fn extension(self) -> ConfigExt {
		Extension(Arc::new(self))
	}
}
