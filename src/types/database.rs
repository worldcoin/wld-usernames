use crate::types::{Address, VerificationLevel};
use chrono::Utc;
use sqlx::prelude::FromRow;
use sqlxinsert::PgInsert;

/// A registered username.
#[derive(Debug, FromRow, PgInsert)]
pub struct Name {
	/// Checksummed address of the owner.
	pub address: String,
	/// World App username of the owner.
	pub username: String,
	/// The nullifier hash of the proof that was used to register this name.
	pub nullifier_hash: String,
	/// The verificaiton level of the proof that was used to register this name.
	pub verification_level: String,
	/// The time at which this name was registered.
	pub created_at: chrono::NaiveDateTime,
	/// The time at which this name was last updated.
	pub updated_at: chrono::NaiveDateTime,
}

impl Name {
	pub fn new(
		username: String,
		address: &Address,
		nullifier_hash: String,
		verification_level: &VerificationLevel,
	) -> Self {
		Self {
			username,
			nullifier_hash,
			created_at: Utc::now().naive_utc(),
			updated_at: Utc::now().naive_utc(),
			address: address.0.to_checksum(None),
			verification_level: verification_level.to_string(),
		}
	}
}

#[allow(dead_code)]
#[derive(Debug, FromRow, PgInsert)]
pub struct MovedRecord {
	pub id: f64,
	pub old_username: String,
	pub new_username: String,
}
