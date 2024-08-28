use alloy::primitives::{keccak256, FixedBytes};
use axum::Extension;
use idkit::session::AppId;
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::{
	env::{self, VarError},
	num::ParseIntError,
	sync::Arc,
	time::Duration,
};

use crate::blocklist::{Blocklist, BlocklistExt};

#[allow(clippy::module_name_repetitions)]
pub type ConfigExt = Extension<Arc<Config>>;

pub struct Config {
	pub wld_app_id: AppId,
	pub ens_chain_id: u64,
	pub ens_domain: String,
	pub kms_key_id: String,
	db_client: Option<PgPool>,
	blocklist: Option<Blocklist>,
	kms_client: Option<aws_sdk_kms::Client>,
	pub ens_resolver_salt: FixedBytes<32>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error(transparent)]
	Env(#[from] VarError),
	#[error(transparent)]
	Sqlx(#[from] sqlx::Error),
	#[error(transparent)]
	ChainId(#[from] ParseIntError),
}

impl Config {
	pub async fn from_env() -> Result<Self, Error> {
		let blocklist = Blocklist::new(
			&env::var("RESERVED_USERNAMES")?,
			&env::var("BLOCKED_SUBSTRINGS")?,
		);

		let db_client = PgPoolOptions::new()
			.acquire_timeout(Duration::from_secs(3))
			.connect(&env::var("DATABASE_URL")?)
			.await?;

		let kms_client = aws_sdk_kms::Client::new(&aws_config::load_from_env().await);

		Ok(Self {
			db_client: Some(db_client),
			blocklist: Some(blocklist),
			kms_client: Some(kms_client),
			kms_key_id: env::var("KMS_KEY_ID")?,
			ens_domain: env::var("ENS_DOMAIN")?,
			ens_chain_id: env::var("ENS_CHAIN_ID")?.parse()?,
			ens_resolver_salt: keccak256(env::var("ENS_RESOLVER_SALT")?),
			wld_app_id: unsafe { AppId::new_unchecked(env::var("WLD_APP_ID")?) },
		})
	}

	pub fn db_extension(&mut self) -> Extension<PgPool> {
		Extension(self.db_client.take().unwrap())
	}

	pub fn blocklist_extension(&mut self) -> BlocklistExt {
		Extension(Arc::new(self.blocklist.take().unwrap()))
	}

	pub fn kms_extension(&mut self) -> Extension<aws_sdk_kms::Client> {
		Extension(self.kms_client.take().unwrap())
	}

	pub fn extension(self) -> ConfigExt {
		Extension(Arc::new(self))
	}
}
