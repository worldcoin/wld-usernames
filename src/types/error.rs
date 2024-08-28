#![allow(clippy::module_name_repetitions)]

use aide::{OperationIo, OperationOutput};
use axum::response::IntoResponse;
use axum_jsonschema::Json;
use http::StatusCode;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

#[derive(Debug)]
pub struct ErrorResponse {
    error: String,
    status: StatusCode,
}

impl ErrorResponse {
    pub const fn not_found(error: String) -> Self {
        Self {
            error,
            status: StatusCode::NOT_FOUND,
        }
    }

    pub const fn validation_error(error: String) -> Self {
        Self {
            error,
            status: StatusCode::UNPROCESSABLE_ENTITY,
        }
    }

    pub const fn server_error(error: String) -> Self {
        Self {
            error,
            status: StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl<E: std::error::Error> From<E> for ErrorResponse {
    fn from(err: E) -> Self {
        tracing::error!("{err:?}");

        Self::server_error("Internal Server Error".to_string())
    }
}

impl IntoResponse for ErrorResponse {
    fn into_response(self) -> axum::response::Response {
        (self.status, Json(json! ({ "error": self.error }))).into_response()
    }
}

impl OperationOutput for ErrorResponse {
    type Inner = Self;

    fn operation_response(
        _: &mut aide::gen::GenContext,
        _: &mut aide::openapi::Operation,
    ) -> Option<aide::openapi::Response> {
        None
    }
}

#[derive(Debug, Serialize, Deserialize, JsonSchema, OperationIo)]
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
