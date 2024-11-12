#![allow(clippy::module_name_repetitions)]

use aide::{gen::GenContext, openapi::Operation, OperationOutput};
use axum::response::IntoResponse;
use axum_jsonschema::Json;
use http::StatusCode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug)]
pub struct ErrorResponse {
	error: String,
	status: StatusCode,
}

#[derive(Debug, Serialize, JsonSchema)]
struct ErrorResponseSchema {
	error: String,
}

impl ErrorResponse {
	pub const fn not_found(error: String) -> Self {
		Self {
			error,
			status: StatusCode::NOT_FOUND,
		}
	}

	pub fn unauthorized(error: String) -> Self {
		tracing::error!("Unauthorized: {}", error);
		Self {
			error,
			status: StatusCode::UNAUTHORIZED,
		}
	}

	pub fn validation_error(error: String) -> Self {
		tracing::error!("Validation Error: {}", error);
		Self {
			error,
			status: StatusCode::UNPROCESSABLE_ENTITY,
		}
	}

	pub fn server_error(error: String) -> Self {
		tracing::error!("Internal Server Error: {}", error);
		Self {
			error,
			status: StatusCode::INTERNAL_SERVER_ERROR,
		}
	}
}

impl<E: std::error::Error> From<E> for ErrorResponse {
	fn from(_: E) -> Self {
		Self::server_error("Internal Server Error".to_string())
	}
}

impl IntoResponse for ErrorResponse {
	fn into_response(self) -> axum::response::Response {
		if self.status != StatusCode::NOT_FOUND {
			tracing::error!(error = %self.error, status = ?self.status);
		}
		(self.status, Json(ErrorResponseSchema { error: self.error })).into_response()
	}
}

impl OperationOutput for ErrorResponse {
	type Inner = Self;

	fn operation_response(
		ctx: &mut aide::gen::GenContext,
		operation: &mut aide::openapi::Operation,
	) -> Option<aide::openapi::Response> {
		Json::<ErrorResponseSchema>::operation_response(ctx, operation)
	}
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ENSErrorResponse {
	/// A human-readable error message.
	pub message: String,
}

impl ENSErrorResponse {
	pub fn new(message: &str) -> Self {
		Self {
			message: message.to_string(),
		}
	}
}

impl IntoResponse for ENSErrorResponse {
	fn into_response(self) -> axum::response::Response {
		(StatusCode::BAD_REQUEST, Json(self)).into_response()
	}
}

impl OperationOutput for ENSErrorResponse {
	type Inner = Self;

	fn operation_response(
		ctx: &mut GenContext,
		operation: &mut Operation,
	) -> Option<aide::openapi::Response> {
		Json::<Self>::operation_response(ctx, operation)
	}

	fn inferred_responses(
		ctx: &mut aide::gen::GenContext,
		operation: &mut Operation,
	) -> Vec<(Option<u16>, aide::openapi::Response)> {
		Self::operation_response(ctx, operation).map_or_else(Vec::new, |res| {
			vec![(Some(StatusCode::BAD_REQUEST.as_u16()), res)]
		})
	}
}
