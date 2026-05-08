use aide::transform::TransformOperation;
use axum::{
	extract::Path,
	http::{HeaderMap, StatusCode},
	Extension,
};
use redis::aio::ConnectionManager;
use tracing::{info, instrument, warn};

use crate::{
	config::{ConfigExt, Db},
	data_deletion_worker::{UsernameDeletionService, UsernameDeletionServiceImpl},
	types::ErrorResponse,
};

pub(super) const SKIP_ATTESTATION_HEADER: &str = "x-e2e-skip-attestation";

/// Returns `true` when the request is allowed to use the dev/internal endpoint:
/// the runtime environment must allow skipping attestation (Development or Staging),
/// and the request must carry `x-e2e-skip-attestation: true`.
pub(super) fn is_e2e_skip_allowed(
	allowed_to_skip_attestation: bool,
	headers: &HeaderMap,
) -> bool {
	let header_present = headers
		.get(SKIP_ATTESTATION_HEADER)
		.and_then(|v| v.to_str().ok())
		.is_some_and(|v| v == "true");

	allowed_to_skip_attestation && header_present
}

/// Pure handler logic, decoupled from axum extensions for testability.
///
/// Returns 403 if the request fails the dev/e2e gate, 500 if the deletion
/// service errors, and 204 otherwise. The deletion is idempotent – missing
/// rows are not an error.
async fn handle_delete_username(
	service: &dyn UsernameDeletionService,
	allowed_to_skip_attestation: bool,
	headers: &HeaderMap,
	address: &str,
) -> Result<StatusCode, ErrorResponse> {
	if !is_e2e_skip_allowed(allowed_to_skip_attestation, headers) {
		warn!("Dev delete_username endpoint called without valid e2e skip context");
		return Err(ErrorResponse::forbidden(
			"This endpoint is only available in dev/e2e contexts.".to_string(),
		));
	}

	service.delete_username(address).await.map_err(|e| {
		tracing::error!(
			"Dev delete_username failed for {}: {}",
			address,
			e.to_string()
		);
		ErrorResponse::server_error("Failed to delete username record".to_string())
	})?;

	info!(address = %address, "Dev/internal delete_username succeeded");
	Ok(StatusCode::NO_CONTENT)
}

/// Dev/internal endpoint: deletes a username record by wallet address.
///
/// Intended for use by app-backend e2e tests to clean up rows created via
/// `POST /v1/usernames/register`. It is gated by the same dev/e2e config flag
/// pattern used for `x-e2e-skip-attestation`: callers must run in Development
/// or Staging **and** send `x-e2e-skip-attestation: true`. Any other case
/// returns 403 Forbidden.
///
/// On success returns 204 No Content. The underlying delete is idempotent –
/// missing rows are not an error.
#[instrument(skip_all, fields(wallet_address = %address))]
pub async fn delete_username(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(redis): Extension<ConnectionManager>,
	headers: HeaderMap,
	Path(address): Path<String>,
) -> Result<StatusCode, ErrorResponse> {
	let service = UsernameDeletionServiceImpl::new(db.read_write.clone(), redis.clone());
	handle_delete_username(
		&service,
		config.allowed_to_skip_attestation(),
		&headers,
		&address,
	)
	.await
}

pub fn docs(op: TransformOperation) -> TransformOperation {
	op.description(
		"Dev/internal endpoint to delete a username record by wallet address. \
		 Available only in Development/Staging environments and only when the \
		 `x-e2e-skip-attestation: true` header is present. Intended for e2e \
		 test cleanup; idempotent.",
	)
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::data_deletion_worker::QueueError;
	use async_trait::async_trait;
	use axum::http::HeaderValue;
	use axum::response::IntoResponse;
	use std::sync::{
		atomic::{AtomicUsize, Ordering},
		Arc,
	};

	#[test]
	fn rejects_when_env_not_allowed() {
		let mut headers = HeaderMap::new();
		headers.insert(SKIP_ATTESTATION_HEADER, HeaderValue::from_static("true"));
		assert!(!is_e2e_skip_allowed(false, &headers));
	}

	#[test]
	fn rejects_when_header_missing() {
		let headers = HeaderMap::new();
		assert!(!is_e2e_skip_allowed(true, &headers));
	}

	#[test]
	fn rejects_when_header_is_not_true() {
		let mut headers = HeaderMap::new();
		headers.insert(SKIP_ATTESTATION_HEADER, HeaderValue::from_static("false"));
		assert!(!is_e2e_skip_allowed(true, &headers));
	}

	#[test]
	fn rejects_when_header_is_empty() {
		let mut headers = HeaderMap::new();
		headers.insert(SKIP_ATTESTATION_HEADER, HeaderValue::from_static(""));
		assert!(!is_e2e_skip_allowed(true, &headers));
	}

	#[test]
	fn allows_when_env_and_header_set() {
		let mut headers = HeaderMap::new();
		headers.insert(SKIP_ATTESTATION_HEADER, HeaderValue::from_static("true"));
		assert!(is_e2e_skip_allowed(true, &headers));
	}

	#[derive(Default)]
	struct MockService {
		calls: AtomicUsize,
		fail: bool,
	}

	#[async_trait]
	impl UsernameDeletionService for MockService {
		async fn delete_username(&self, _wallet_address: &str) -> Result<(), QueueError> {
			self.calls.fetch_add(1, Ordering::SeqCst);
			if self.fail {
				Err(QueueError::CacheInvalidationError("mock failure".into()))
			} else {
				Ok(())
			}
		}
	}

	fn headers_with_skip(value: &'static str) -> HeaderMap {
		let mut headers = HeaderMap::new();
		headers.insert(SKIP_ATTESTATION_HEADER, HeaderValue::from_static(value));
		headers
	}

	#[tokio::test]
	async fn handler_returns_forbidden_in_production_even_with_header() {
		let service = Arc::new(MockService::default());
		let headers = headers_with_skip("true");

		let result = handle_delete_username(
			service.as_ref(),
			false,
			&headers,
			"0x0000000000000000000000000000000000000001",
		)
		.await;

		let response = result.expect_err("should be forbidden").into_response();
		assert_eq!(response.status(), StatusCode::FORBIDDEN);
		assert_eq!(
			service.calls.load(Ordering::SeqCst),
			0,
			"deletion service must not be called when forbidden"
		);
	}

	#[tokio::test]
	async fn handler_returns_forbidden_in_dev_without_header() {
		let service = Arc::new(MockService::default());
		let headers = HeaderMap::new();

		let result = handle_delete_username(
			service.as_ref(),
			true,
			&headers,
			"0x0000000000000000000000000000000000000001",
		)
		.await;

		let response = result.expect_err("should be forbidden").into_response();
		assert_eq!(response.status(), StatusCode::FORBIDDEN);
		assert_eq!(service.calls.load(Ordering::SeqCst), 0);
	}

	#[tokio::test]
	async fn handler_calls_service_and_returns_no_content_on_success() {
		let service = Arc::new(MockService::default());
		let headers = headers_with_skip("true");

		let result = handle_delete_username(
			service.as_ref(),
			true,
			&headers,
			"0xabCDEF0000000000000000000000000000000123",
		)
		.await;

		assert_eq!(result.unwrap(), StatusCode::NO_CONTENT);
		assert_eq!(service.calls.load(Ordering::SeqCst), 1);
	}

	#[tokio::test]
	async fn handler_returns_server_error_when_service_fails() {
		let service = Arc::new(MockService {
			calls: AtomicUsize::new(0),
			fail: true,
		});
		let headers = headers_with_skip("true");

		let result = handle_delete_username(
			service.as_ref(),
			true,
			&headers,
			"0x0000000000000000000000000000000000000001",
		)
		.await;

		let response = result.expect_err("should be server error").into_response();
		assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
		assert_eq!(service.calls.load(Ordering::SeqCst), 1);
	}
}
