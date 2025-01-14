use crate::config::DebugClusterClient;
use axum::Extension;
use axum_jsonschema::Json;
use redis::{Commands, RedisResult};

use crate::{
	config::Db,
	types::{ErrorResponse, Name, QueryAddressesPayload, UsernameRecord},
	utils::ONE_MINUTE_IN_SECONDS,
};

pub async fn query_multiple(
	Extension(db): Extension<Db>,
	Extension(redis): Extension<DebugClusterClient>,
	Json(payload): Json<QueryAddressesPayload>,
) -> Result<Json<Vec<UsernameRecord>>, ErrorResponse> {
	let addresses = payload
		.addresses
		.iter()
		.map(|a| a.0.to_checksum(None))
		.collect::<Vec<_>>();

	if addresses.len() > 50 {
		return Err(ErrorResponse::validation_error(
			"Too many addresses, max is 50".to_string(),
		));
	}

	let names = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE address = ANY($1)",
		&addresses
	)
	.fetch_all(&db.read_only)
	.await?;

	let records: Vec<UsernameRecord> = names.into_iter().map(UsernameRecord::from).collect();
	let cache_key = format!("query_multiple:{:?}", addresses.join(";"));
	let mut conn = redis.client.get_connection().unwrap();

	if let Ok(json_data) = serde_json::to_string(&records) {
		let _: RedisResult<()> = conn.set_ex(&cache_key, json_data, ONE_MINUTE_IN_SECONDS * 5);
	}

	Ok(Json(records))
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Resolve multiple addresses into their registered usernames.")
		.response_with::<422, ErrorResponse, _>(|op| {
			op.description("There were too many addresses")
		})
}
