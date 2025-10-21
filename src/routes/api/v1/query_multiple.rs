use std::collections::HashMap;

use axum::Extension;
use axum_jsonschema::Json;
use tracing::{info_span, Instrument};

use crate::{
	config::Db,
	types::{ErrorResponse, Name, QueryMultiplePayload, UsernameRecord},
};

pub async fn query_multiple(
	Extension(db): Extension<Db>,
	Json(payload): Json<QueryMultiplePayload>,
) -> Result<Json<Vec<UsernameRecord>>, ErrorResponse> {
	let addresses = payload
		.addresses
		.iter()
		.map(|a| a.0.to_checksum(None))
		.collect::<Vec<_>>();

	let usernames = payload
		.usernames
		.iter()
		.map(|u| u.to_lowercase())
		.collect::<Vec<_>>();

	if addresses.is_empty() && usernames.is_empty() {
		return Ok(Json(Vec::new()));
	}

	let mut names_by_address: HashMap<String, Name> = HashMap::new();

	if !addresses.is_empty() {
		let address_matches = sqlx::query_as!(
			Name,
			"SELECT * FROM names WHERE address = ANY($1::text[])",
			&addresses
		)
		.fetch_all(&db.read_only)
		.instrument(info_span!(
			"query_multiple_address_query",
			addresses = addresses.len()
		))
		.await?;

		for name in address_matches {
			names_by_address.entry(name.address.clone()).or_insert(name);
		}
	}

	if !usernames.is_empty() {
		let username_matches = sqlx::query_as!(
			Name,
			"SELECT * FROM names WHERE LOWER(username) = ANY($1::text[])",
			&usernames
		)
		.fetch_all(&db.read_only)
		.instrument(info_span!(
			"query_multiple_username_query",
			usernames = usernames.len()
		))
		.await?;

		for name in username_matches {
			names_by_address.entry(name.address.clone()).or_insert(name);
		}
	}

	let records_json: Vec<UsernameRecord> = names_by_address
		.into_values()
		.map(UsernameRecord::from)
		.collect();

	Ok(Json(records_json))
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description(
		"Resolve multiple addresses or usernames into their registered username records.",
	)
	.response_with::<422, ErrorResponse, _>(|op| op.description("There were too many items"))
}
