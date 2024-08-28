use idkit::Proof;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Address, VerificationLevel};

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RegisterUsernamePayload {
    proof: String,
    merkle_root: String,
    pub username: String,
    pub address: Address,
    pub nullifier_hash: String,
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
    pub addresses: Vec<Address>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ENSQueryPayload {
    pub data: String,
    pub sender: Address,
}
