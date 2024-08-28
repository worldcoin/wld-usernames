use crate::types::{Address, VerificationLevel};
use schemars::JsonSchema;
use serde::Serialize;
use sqlx::prelude::FromRow;
use sqlxinsert::PgInsert;

#[derive(Debug, Serialize, FromRow, PgInsert, JsonSchema)]
pub struct Name {
    pub address: String,
    pub username: String,
    pub nullifier_hash: String,
    pub verification_level: String,
    pub created_at: Option<chrono::NaiveDateTime>,
    pub updated_at: Option<chrono::NaiveDateTime>,
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
            created_at: None,
            updated_at: None,
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
