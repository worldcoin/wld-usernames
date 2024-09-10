use http::StatusCode;

use crate::types::ErrorResponse;

pub async fn health() -> Result<StatusCode, ErrorResponse> {
	Ok(StatusCode::OK)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Health check.")
}
