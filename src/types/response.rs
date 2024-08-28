#![allow(clippy::module_name_repetitions)]

use aide::OperationIo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Address, Name};

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
}

#[allow(clippy::fallible_impl_from)]
impl From<Name> for UsernameRecord {
    fn from(value: Name) -> Self {
        Self {
            username: value.username,
            address: Address(value.address.parse().unwrap()),
        }
    }
}
