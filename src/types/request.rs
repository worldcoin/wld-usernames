use idkit::Proof;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Address, VerificationLevel};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RegisterUsernamePayload {
    /// 0x-prefixed hex string of the World ID proof.
    proof: String,
    /// 0x-prefixed hex string of the World ID merkle root.
    merkle_root: String,
    /// The requested username.
    pub username: String,
    /// The user's walle address.
    pub address: Address,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ENSQueryPayload {
    pub data: String,
    pub sender: Address,
}
