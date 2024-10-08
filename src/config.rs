use anyhow::Context;
use axum::Extension;
use idkit::session::AppId;
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

#[derive(Debug)]
pub struct Config {
	pub wld_app_id: AppId,
	pub ens_chain_id: u64,
	pub ens_domain: String,
	pub private_key: String,
	pub developer_portal_url: String,
	db_client: Option<PgPool>,
	blocklist: Option<Blocklist>,
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
			.acquire_timeout(Duration::from_secs(3))
			.connect(
				&env::var("DATABASE_URL").context("DATABASE_URL environment variable not set")?,
			)
			.await?;

		Ok(Self {
			db_client: Some(db_client),
			blocklist: Some(blocklist),
			ens_domain: env::var("ENS_DOMAIN")
				.context("ENS_DOMAIN environment variable not set")?,
			private_key: env::var("PRIVATE_KEY")
				.context("PRIVATE_KEY environment variable not set")?,
			ens_chain_id: env::var("ENS_CHAIN_ID")
				.context("ENS_CHAIN_ID environment variable not set")?
				.parse()
				.context("ENS_CHAIN_ID could not be parsed as a number")?,
			wld_app_id: unsafe {
				AppId::new_unchecked(
					env::var("WLD_APP_ID").context("WLD_APP_ID environment variable not set")?,
				)
			},
			developer_portal_url: env::var("DEVELOPER_PORTAL_ENDPOINT")
				.context("DEVELOPER_PORTAL_ENDPOINT environment variable not set")?,
		})
	}

	pub async fn migrate_database(&self) -> Result<(), MigrateError> {
		sqlx::migrate!().run(self.db_client.as_ref().unwrap()).await
	}

	pub fn db_extension(&mut self) -> Extension<PgPool> {
		Extension(self.db_client.take().unwrap())
	}

	pub fn blocklist_extension(&mut self) -> BlocklistExt {
		Extension(Arc::new(self.blocklist.take().unwrap()))
	}

	pub fn extension(self) -> ConfigExt {
		Extension(Arc::new(self))
	}
}
