use crate::types::{Address, VerificationLevel};
use chrono::Utc;
use sqlx::prelude::FromRow;
use sqlxinsert::PgInsert;
use url::Url;

/// A registered username.
#[derive(Debug, FromRow, PgInsert)]
pub struct Name {
	/// Check-summed address of the owner.
	pub address: String,
	/// World App username of the owner.
	pub username: String,
	/// URL of the owner's profile picture.
	pub profile_picture_url: Option<String>,
	/// URL of the owner's minimized profile picture.
	pub minimized_profile_picture_url: Option<String>,
	/// The nullifier hash of the proof that was used to register this name.
	pub nullifier_hash: String,
	/// The verification level of the proof that was used to register this name.
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
		profile_picture_url: Option<Url>,
		minimized_profile_picture_url: Option<Url>,
		nullifier_hash: String,
		verification_level: &VerificationLevel,
	) -> Self {
		Self {
			username,
			nullifier_hash,
			created_at: Utc::now().naive_utc(),
			updated_at: Utc::now().naive_utc(),
			address: address.to_checksum(None),
			verification_level: verification_level.to_string(),
			profile_picture_url: profile_picture_url.map(|u| u.to_string()),
			minimized_profile_picture_url: minimized_profile_picture_url.map(|u| u.to_string()),
		}
	}
}

#[allow(dead_code)]
#[derive(Debug, FromRow, PgInsert)]
pub struct MovedRecord {
	pub old_username: String,
	pub new_username: String,
}
#[derive(Debug, sqlx::FromRow)] // Add other derives as needed like Debug, Clone, etc.
pub struct NameSearch {
	pub username: String,
	pub address: String,
	pub profile_picture_url: Option<String>,
	pub minimized_profile_picture_url: Option<String>,
}

#[derive(Debug, sqlx::FromRow)] // Add other derives as needed like Debug, Clone, etc.
pub struct MovedAddress {
	pub address: String,
}
