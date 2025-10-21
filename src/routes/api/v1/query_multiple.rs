use std::collections::HashSet;

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
	tracing::info!(
		"query_multiple called with {} addresses and {} usernames",
		payload.addresses.len(),
		payload.usernames.len()
	);

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

	tracing::info!("Processing {} addresses: {:?}", addresses.len(), addresses);
	tracing::info!("Processing {} usernames: {:?}", usernames.len(), usernames);

	if addresses.is_empty() && usernames.is_empty() {
		return Ok(Json(Vec::new()));
	}

	let mut names: Vec<Name> = Vec::new();
	let mut seen_usernames = HashSet::new();

	if !addresses.is_empty() {
		tracing::info!("Querying database for addresses...");
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
			if seen_usernames.insert(name.username.clone()) {
				names.push(name);
			}
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
			if seen_usernames.insert(name.username.clone()) {
				names.push(name);
			}
		}
	}

	let records_json: Vec<UsernameRecord> = names.into_iter().map(UsernameRecord::from).collect();

	Ok(Json(records_json))
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description(
		"Resolve multiple addresses or usernames into their registered username records.",
	)
	.response_with::<422, ErrorResponse, _>(|op| op.description("There were too many items"))
}
