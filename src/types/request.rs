use idkit::Proof;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use url::Url;

use super::{Address, VerificationLevel};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RegisterUsernamePayload {
	/// 0x-prefixed hex string of the World ID proof.
	proof: String,
	/// 0x-prefixed hex string of the World ID merkle root.
	merkle_root: String,
	/// The requested username.
	pub username: String,
	/// The user's wallet address.
	pub address: Address,
	/// The user's profile picture URL.
	pub profile_picture_url: Option<Url>,
	/// The user's minimized profile picture URL.
	pub minimized_profile_picture_url: Option<Url>,
	/// 0x-prefixed hex string of the World ID nullifier hash.
	pub nullifier_hash: String,
	/// World ID verification level the user holds.
	pub verification_level: VerificationLevel,
}

impl RegisterUsernamePayload {
	#[allow(clippy::wrong_self_convention)]
	pub fn into_proof(&self) -> Proof {
		Proof {
			proof: self.proof.clone(),
			merkle_root: self.merkle_root.clone(),
			nullifier_hash: self.nullifier_hash.clone(),
			verification_level: self.verification_level.0,
		}
	}
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct QueryAddressesPayload {
	/// A list of addresses to resolve.
	pub addresses: Vec<Address>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct AvatarQueryParams {
	/// The URL to redirect to if the username is not found or does not have a profile picture URL.
	pub fallback: Option<Url>,
	/// Whether to return the minimized version of the profile picture. Defaults to false.
	pub minimized: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct UpdateUsernamePayload {
	/// 0x-prefixed hex string of the World ID proof.
	proof: String,
	/// 0x-prefixed hex string of the World ID merkle root.
	merkle_root: String,
	/// The username's new wallet address.
	pub address: Address,
	/// The username's new profile picture URL. If not provided, the exixting profile picture URL will be deleted.
	pub profile_picture_url: Option<Url>,
	/// The username's new minimized profile picture URL. If not provided, the existing minimized profile picture URL will be deleted.
	pub minimized_profile_picture_url: Option<Url>,
	/// 0x-prefixed hex string of the World ID nullifier hash.
	pub nullifier_hash: String,
	/// World ID verification level the user holds.
	pub verification_level: VerificationLevel,
}

impl UpdateUsernamePayload {
	#[allow(clippy::wrong_self_convention)]
	pub fn into_proof(&self) -> Proof {
		Proof {
			proof: self.proof.clone(),
			merkle_root: self.merkle_root.clone(),
			nullifier_hash: self.nullifier_hash.clone(),
			verification_level: self.verification_level.0,
		}
	}
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RenamePayload {
	/// 0x-prefixed hex string of the World ID proof.
	proof: String,
	/// 0x-prefixed hex string of the World ID merkle root.
	merkle_root: String,
	/// The username to migrate from.
	pub old_username: String,
	/// The username to migrate to.
	pub new_username: String,
	/// 0x-prefixed hex string of the World ID nullifier hash.
	pub nullifier_hash: String,
	/// World ID verification level the user holds.
	pub verification_level: VerificationLevel,
}

impl RenamePayload {
	#[allow(clippy::wrong_self_convention)]
	pub fn into_proof(&self) -> Proof {
		Proof {
			proof: self.proof.clone(),
			merkle_root: self.merkle_root.clone(),
			nullifier_hash: self.nullifier_hash.clone(),
			verification_level: self.verification_level.0,
		}
	}
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ENSQueryPayload {
	pub data: String,
	pub sender: Address,
}
