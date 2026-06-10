use aide::transform::TransformOperation;
use axum::{
	extract::Path,
	http::{HeaderMap, StatusCode},
	Extension,
};
use redis::aio::ConnectionManager;
use subtle::ConstantTimeEq;
use tracing::{info, instrument, warn};

use crate::{
	config::{ConfigExt, Db},
	data_deletion_worker::{UsernameDeletionService, UsernameDeletionServiceImpl},
	types::ErrorResponse,
};

pub(super) const SKIP_ATTESTATION_HEADER: &str = "x-e2e-skip-attestation";
pub(super) const INTERNAL_API_SECRET_HEADER: &str = "x-internal-api-secret";

/// Returns `true` when the request is allowed to use the dev/internal endpoint:
/// the runtime environment must allow skipping attestation (Development or Staging),
/// and the request must carry `x-e2e-skip-attestation: true`.
pub(super) fn is_e2e_skip_allowed(
	allowed_to_skip_attestation: bool,
	expected_secret: Option<&str>,
	headers: &HeaderMap,
) -> bool {
	let header_present = headers
		.get(SKIP_ATTESTATION_HEADER)
		.and_then(|v| v.to_str().ok())
		.is_some_and(|v| v == "true");

	let provided_secret = headers
		.get(INTERNAL_API_SECRET_HEADER)
		.and_then(|v| v.to_str().ok());

	let secret_ok = match (expected_secret, provided_secret) {
		(Some(expected), Some(provided)) if !expected.is_empty() => {
			expected.as_bytes().ct_eq(provided.as_bytes()).into()
		},
		_ => false,
	};

	allowed_to_skip_attestation && header_present && secret_ok
}

/// Pure handler logic, decoupled from axum extensions for testability.
///
/// Returns 403 if the request fails the dev/e2e gate, 500 if the deletion
/// service errors, and 204 otherwise. The deletion is idempotent – missing
/// rows are not an error.
async fn handle_delete_username(
	service: &dyn UsernameDeletionService,
	allowed_to_skip_attestation: bool,
	expected_secret: Option<&str>,
	headers: &HeaderMap,
	address: &str,
) -> Result<StatusCode, ErrorResponse> {
	if !is_e2e_skip_allowed(allowed_to_skip_attestation, expected_secret, headers) {
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
		config.internal_api_secret(),
		&headers,
		&address,
	)
	.await
}

pub fn docs(op: TransformOperation) -> TransformOperation {
	op.description(
		"Dev/internal endpoint to delete a username record by wallet address. \
		 Available only in Development/Staging environments and only when both \
		 `x-e2e-skip-attestation: true` and a matching `x-internal-api-secret` \
		 header are present. Intended for e2e test cleanup; idempotent.",
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

	const TEST_SECRET: &str = "test-internal-secret-do-not-use-in-prod";

	fn headers_with(skip: Option<&'static str>, secret: Option<&'static str>) -> HeaderMap {
		let mut headers = HeaderMap::new();
		if let Some(v) = skip {
			headers.insert(SKIP_ATTESTATION_HEADER, HeaderValue::from_static(v));
		}
		if let Some(v) = secret {
			headers.insert(INTERNAL_API_SECRET_HEADER, HeaderValue::from_static(v));
		}
		headers
	}

	#[test]
	fn rejects_when_env_not_allowed() {
		let headers = headers_with(Some("true"), Some(TEST_SECRET));
		assert!(!is_e2e_skip_allowed(false, Some(TEST_SECRET), &headers));
	}

	#[test]
	fn rejects_when_skip_header_missing() {
		let headers = headers_with(None, Some(TEST_SECRET));
		assert!(!is_e2e_skip_allowed(true, Some(TEST_SECRET), &headers));
	}

	#[test]
	fn rejects_when_skip_header_is_not_true() {
		let headers = headers_with(Some("false"), Some(TEST_SECRET));
		assert!(!is_e2e_skip_allowed(true, Some(TEST_SECRET), &headers));
	}

	#[test]
	fn rejects_when_skip_header_is_empty() {
		let headers = headers_with(Some(""), Some(TEST_SECRET));
		assert!(!is_e2e_skip_allowed(true, Some(TEST_SECRET), &headers));
	}

	#[test]
	fn rejects_when_secret_header_missing() {
		let headers = headers_with(Some("true"), None);
		assert!(!is_e2e_skip_allowed(true, Some(TEST_SECRET), &headers));
	}

	#[test]
	fn rejects_when_secret_header_does_not_match() {
		let headers = headers_with(Some("true"), Some("nope"));
		assert!(!is_e2e_skip_allowed(true, Some(TEST_SECRET), &headers));
	}

	#[test]
	fn rejects_when_secret_header_is_empty() {
		let headers = headers_with(Some("true"), Some(""));
		assert!(!is_e2e_skip_allowed(true, Some(TEST_SECRET), &headers));
	}

	#[test]
	fn rejects_when_expected_secret_is_none_even_if_header_present() {
		let headers = headers_with(Some("true"), Some(TEST_SECRET));
		assert!(
			!is_e2e_skip_allowed(true, None, &headers),
			"unset INTERNAL_API_SECRET must fail closed"
		);
	}

	#[test]
	fn rejects_when_expected_secret_is_empty_string() {
		let headers = headers_with(Some("true"), Some(""));
		assert!(
			!is_e2e_skip_allowed(true, Some(""), &headers),
			"empty expected secret must never authorize, even against an empty header"
		);
	}

	#[test]
	fn allows_when_env_and_both_headers_set_and_secret_matches() {
		let headers = headers_with(Some("true"), Some(TEST_SECRET));
		assert!(is_e2e_skip_allowed(true, Some(TEST_SECRET), &headers));
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

	#[tokio::test]
	async fn handler_returns_forbidden_in_production_even_with_headers() {
		let service = Arc::new(MockService::default());
		let headers = headers_with(Some("true"), Some(TEST_SECRET));

		let result = handle_delete_username(
			service.as_ref(),
			false,
			Some(TEST_SECRET),
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
	async fn handler_returns_forbidden_in_dev_without_headers() {
		let service = Arc::new(MockService::default());
		let headers = HeaderMap::new();

		let result = handle_delete_username(
			service.as_ref(),
			true,
			Some(TEST_SECRET),
			&headers,
			"0x0000000000000000000000000000000000000001",
		)
		.await;

		let response = result.expect_err("should be forbidden").into_response();
		assert_eq!(response.status(), StatusCode::FORBIDDEN);
		assert_eq!(service.calls.load(Ordering::SeqCst), 0);
	}

	#[tokio::test]
	async fn handler_returns_forbidden_when_secret_missing_in_config() {
		let service = Arc::new(MockService::default());
		let headers = headers_with(Some("true"), Some(TEST_SECRET));

		let result = handle_delete_username(
			service.as_ref(),
			true,
			None,
			&headers,
			"0x0000000000000000000000000000000000000001",
		)
		.await;

		let response = result.expect_err("should be forbidden").into_response();
		assert_eq!(response.status(), StatusCode::FORBIDDEN);
		assert_eq!(
			service.calls.load(Ordering::SeqCst),
			0,
			"deletion must not occur when INTERNAL_API_SECRET is unset"
		);
	}

	#[tokio::test]
	async fn handler_returns_forbidden_when_secret_does_not_match() {
		let service = Arc::new(MockService::default());
		let headers = headers_with(Some("true"), Some("wrong-secret"));

		let result = handle_delete_username(
			service.as_ref(),
			true,
			Some(TEST_SECRET),
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
		let headers = headers_with(Some("true"), Some(TEST_SECRET));

		let result = handle_delete_username(
			service.as_ref(),
			true,
			Some(TEST_SECRET),
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
		let headers = headers_with(Some("true"), Some(TEST_SECRET));

		let result = handle_delete_username(
			service.as_ref(),
			true,
			Some(TEST_SECRET),
			&headers,
			"0x0000000000000000000000000000000000000001",
		)
		.await;

		let response = result.expect_err("should be server error").into_response();
		assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
		assert_eq!(service.calls.load(Ordering::SeqCst), 1);
	}
}
