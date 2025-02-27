#![allow(clippy::module_name_repetitions)]

use aide::OperationIo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use super::{Address, Name, NameSearch};

#[derive(Debug, Serialize, Deserialize, JsonSchema, OperationIo)]
pub struct ENSResponse {
	/// 0x-prefixed hex string containing the result data.
	pub data: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UsernameRecord {
	/// The user's World App username.
	pub username: String,
	/// Checksummed wallet address of the user.
	pub address: Address,
	/// URL to the user's profile picture.
	pub profile_picture_url: Option<Url>,
	/// The time at which this record was last updated.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub updated_at: Option<chrono::NaiveDateTime>,
}

#[allow(clippy::fallible_impl_from)]
impl From<Name> for UsernameRecord {
	fn from(value: Name) -> Self {
		Self {
			username: value.username,
			address: Address(value.address.parse().unwrap()),
			profile_picture_url: value.profile_picture_url.map(|url| url.parse().unwrap()),
			updated_at: None,
		}
	}
}

#[allow(clippy::fallible_impl_from)]
impl From<NameSearch> for UsernameRecord {
	fn from(value: NameSearch) -> Self {
		Self {
			username: value.username,
			address: Address(value.address.parse().unwrap()),
			profile_picture_url: value.profile_picture_url.map(|url| url.parse().unwrap()),
			updated_at: None,
		}
	}
}
