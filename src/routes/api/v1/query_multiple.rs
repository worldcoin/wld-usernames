use axum::Extension;
use axum_jsonschema::Json;
use sqlx::PgPool;

use crate::types::{ErrorResponse, Name, QueryAddressesPayload, UsernameRecord};

pub async fn query_multiple(
	Extension(db): Extension<PgPool>,
	Json(payload): Json<QueryAddressesPayload>,
) -> Result<Json<Vec<UsernameRecord>>, ErrorResponse> {
	let addresses = payload
		.addresses
		.iter()
		.map(|a| a.0.to_checksum(None))
		.collect::<Vec<_>>();

	let names = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE address = ANY($1)",
		&addresses
	)
	.fetch_all(&db)
	.await?;

	Ok(Json(names.into_iter().map(UsernameRecord::from).collect()))
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Resolve multiple addresses into their registered usernames.")
}
