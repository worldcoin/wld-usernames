use axum::Extension;
use axum_jsonschema::Json;
use tracing::{instrument, Span};

use crate::{
	config::Db,
	types::{ErrorResponse, Name, QueryAddressesPayload, UsernameRecord},
};

pub async fn query_multiple(
	Extension(db): Extension<Db>,
	Json(payload): Json<QueryAddressesPayload>,
) -> Result<Json<Vec<UsernameRecord>>, ErrorResponse> {
	let addresses = payload
		.addresses
		.iter()
		.map(|a| a.0.to_checksum(None))
		.collect::<Vec<_>>();

	Span::current().record("address_count", addresses.len());

	let names = query_multiple_db_query(addresses, &db).await?;

	let records_json: Vec<UsernameRecord> = names.into_iter().map(UsernameRecord::from).collect();

	Ok(Json(records_json))
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Resolve multiple addresses into their registered usernames.")
		.response_with::<422, ErrorResponse, _>(|op| {
			op.description("There were too many addresses")
		})
}

#[instrument(skip_all)]
pub async fn query_multiple_db_query(
	addresses: Vec<String>,
	db: &Db,
) -> Result<Vec<Name>, ErrorResponse> {
	Ok(sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE address = ANY($1)",
		&addresses
	)
	.fetch_all(&db.read_only)
	.await?)
}
