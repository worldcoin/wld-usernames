#![allow(clippy::module_name_repetitions)]

use aide::OperationIo;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, JsonSchema, OperationIo)]
pub struct ENSResponse {
    /// 0x-prefixed hex string containing the result data.
    pub data: String,
}
