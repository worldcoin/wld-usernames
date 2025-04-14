use schemars::{schema, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{fmt, fmt::Display, ops::Deref, str::FromStr};

/// 0x-prefixed hex string representing an Ethereum address.
#[repr(transparent)]
#[derive(Debug, Serialize, Deserialize)]
pub struct Address(pub alloy::primitives::Address);

impl Address {
	pub fn from_string(s: &str) -> anyhow::Result<Self> {
		let address = alloy::primitives::Address::from_str(s)
			.map_err(|e| anyhow::anyhow!("Failed to parse address: {}", e))?;
		Ok(Self(address))
	}
}

impl Deref for Address {
	type Target = alloy::primitives::Address;

	fn deref(&self) -> &Self::Target {
		&self.0
	}
}

impl JsonSchema for Address {
	fn schema_name() -> String {
		"Address".to_string()
	}

	fn json_schema(_: &mut schemars::gen::SchemaGenerator) -> schema::Schema {
		schema::Schema::Object(schema::SchemaObject {
			string: Some(Box::new(schema::StringValidation {
				pattern: Some("^0x[a-fA-F0-9]{40}$".to_string()),
				..Default::default()
			})),
			instance_type: Some(schema::SingleOrVec::Single(Box::new(
				schema::InstanceType::String,
			))),
			..Default::default()
		})
	}
}

/// World ID verification level
#[repr(transparent)]
#[derive(Debug, Serialize, Deserialize)]
pub struct VerificationLevel(pub idkit::session::VerificationLevel);

impl Display for VerificationLevel {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> std::fmt::Result {
		self.0.fmt(f)
	}
}

impl JsonSchema for VerificationLevel {
	fn schema_name() -> String {
		"VerificationLevel".to_string()
	}

	fn json_schema(_: &mut schemars::gen::SchemaGenerator) -> schema::Schema {
		schema::Schema::Object(schema::SchemaObject {
			enum_values: Some(vec![
				Value::String("orb".to_string()),
				Value::String("device".to_string()),
			]),
			instance_type: Some(schema::SingleOrVec::Single(Box::new(
				schema::InstanceType::String,
			))),
			..Default::default()
		})
	}
}
