use schemars::schema;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fmt;
use std::{fmt::Display, ops::Deref};

#[repr(transparent)]
#[derive(Debug, Serialize, Deserialize)]
pub struct Address(pub alloy_primitives::Address);

impl Deref for Address {
    type Target = alloy_primitives::Address;

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