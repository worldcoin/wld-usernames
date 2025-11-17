use super::*;
use crate::config::{Config, Environment};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{header, Method, StatusCode};
use axum::routing::post;
use axum::{Extension, Router};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use p256::ecdsa::SigningKey;
use p256::pkcs8::EncodePrivateKey;
use redis::aio::ConnectionManager;
use sha2::{Digest, Sha256};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Generate a P256 keypair and corresponding JWK for ES256
fn generate_es256_keypair_and_jwk(kid: &str) -> (SigningKey, serde_json::Value) {
	// Generate a random P256 keypair
	let signing_key = SigningKey::random(&mut rand::thread_rng());
	let verifying_key = signing_key.verifying_key();

	// Get the public key point
	let point = verifying_key.to_encoded_point(false);
	let x_bytes = point.x().unwrap();
	let y_bytes = point.y().unwrap();

	// Create JWK with ES256 parameters
	let jwk = serde_json::json!({
		"kty": "EC",
		"crv": "P-256",
		"alg": "ES256",
		"kid": kid,
		"use": "sig",
		"x": URL_SAFE_NO_PAD.encode(x_bytes),
		"y": URL_SAFE_NO_PAD.encode(y_bytes)
	});

	(signing_key, jwk)
}

/// Create a signed JWT with ES256
fn create_test_jwt(signing_key: &SigningKey, kid: &str, jti: String) -> String {
	use jsonwebtoken::{encode, Algorithm, EncodingKey, Header};

	let mut header = Header::new(Algorithm::ES256);
	header.kid = Some(kid.to_string());

	let now = SystemTime::now()
		.duration_since(UNIX_EPOCH)
		.unwrap()
		.as_secs() as i64;

	let claims = serde_json::json!({
		"jti": jti,
		"iat": now,
		"iss": "attestation.worldcoin.org",
		"exp": now + 3600, // 1 hour from now
	});

	// Convert P256 signing key to PEM format for jsonwebtoken
	let pem_doc = signing_key.to_pkcs8_der().unwrap();
	let pem_string = format!(
		"-----BEGIN PRIVATE KEY-----\n{}\n-----END PRIVATE KEY-----",
		base64::engine::general_purpose::STANDARD.encode(pem_doc.as_bytes())
	);

	let key = EncodingKey::from_ec_pem(pem_string.as_bytes()).unwrap();
	encode(&header, &claims, &key).unwrap()
}

/// Create multipart/form-data body with metadata
fn create_multipart_body(metadata: &str, boundary: &str) -> Vec<u8> {
	let body = format!(
		"--{boundary}\r\n\
			Content-Disposition: form-data; name=\"metadata\"\r\n\
			\r\n\
			{metadata}\r\n\
			--{boundary}\r\n\
			Content-Disposition: form-data; name=\"profile_picture\"; filename=\"test.jpg\"\r\n\
			Content-Type: image/jpeg\r\n\
			\r\n\
			fake_image_data\r\n\
			--{boundary}--\r\n"
	);
	body.into_bytes()
}

/// Create a mock Redis connection manager for testing
async fn create_test_redis() -> ConnectionManager {
	// Try to connect to real Redis, or create a mock if not available
	// For this test, we'll use the real Redis since the cache should work
	let redis_url =
		std::env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
	let client = redis::Client::open(redis_url).unwrap();
	ConnectionManager::new(client).await.unwrap()
}

/// Create a minimal test config without database dependencies
fn create_test_config(env: Environment) -> Arc<Config> {
	Arc::new(Config::test_config(env))
}

#[tokio::test]
async fn test_attestation_middleware_happy_path() {
	tracing_subscriber::fmt()
		.with_max_level(tracing::Level::DEBUG)
		.try_init()
		.ok();
	// Setup - use unique kid to avoid Redis cache conflicts
	let kid = format!("test-key-{}", uuid::Uuid::new_v4());
	let metadata = r#"{"test": "data", "foo": "bar"}"#;
	let boundary = "----boundary123";

	// Step 1: Start mock JWKS server
	let mock_server = MockServer::start().await;

	// Step 2: Generate ES256 keypair and JWK
	let (signing_key, jwk) = generate_es256_keypair_and_jwk(&kid);

	// Step 3: Create metadata hash for JTI
	let mut hasher = Sha256::new();
	hasher.update(metadata.as_bytes());
	let jti = hex::encode(hasher.finalize());

	// Step 4: Create JWT with matching JTI
	let token = create_test_jwt(&signing_key, &kid, jti.clone());

	// Step 5: Setup mock to return JWKS
	let jwks_response = serde_json::json!({
		"keys": [jwk]
	});

	Mock::given(method("GET"))
		.and(path("/.well-known/jwks.json"))
		.respond_with(ResponseTemplate::new(200).set_body_json(&jwks_response))
		.mount(&mock_server)
		.await;

	// Step 6: Initialize JwksCache with mock server URL
	let redis = create_test_redis().await;
	let jwks_url = format!("{}/.well-known/jwks.json", mock_server.uri());
	let jwks_cache = Arc::new(JwksCache::new(jwks_url, Duration::from_secs(60), redis));

	// Step 7: Create test router with middleware
	// Create config for testing (production mode to enforce attestation)
	let config = create_test_config(Environment::Production);

	let app = Router::new()
		.route("/test", post(|| async { StatusCode::OK }))
		.route_layer(axum::middleware::from_fn(
			|Extension(cfg): Extension<Arc<Config>>,
			 Extension(cache): Extension<Arc<JwksCache>>,
			 headers,
			 request,
			 next| async move { attestation_middleware(cfg, cache, headers, request, next).await },
		))
		.layer(Extension(config.clone()))
		.layer(Extension(jwks_cache.clone()));

	// Step 8: Create multipart request body
	let body_data = create_multipart_body(metadata, boundary);

	// Step 9: Make request through the router
	let request = Request::builder()
		.method(Method::POST)
		.uri("/test")
		.header(
			header::CONTENT_TYPE,
			format!("multipart/form-data; boundary={}", boundary),
		)
		.header("attestation-gateway-token", &token)
		.body(Body::from(body_data))
		.unwrap();

	let response = app.oneshot(request).await.unwrap();

	// Step 10: Assert success
	assert_eq!(
		response.status(),
		StatusCode::OK,
		"Middleware should allow valid request"
	);
}

#[tokio::test]
async fn test_attestation_middleware_invalid_jti() {
	// Setup similar to happy path but with wrong JTI - use unique kid
	let kid = format!("test-key-{}", uuid::Uuid::new_v4());
	let metadata = r#"{"test": "data"}"#;
	let boundary = "----boundary123";

	let mock_server = MockServer::start().await;
	let (signing_key, jwk) = generate_es256_keypair_and_jwk(&kid);

	// Create JWT with WRONG JTI
	let wrong_jti = "incorrect_hash_value";
	let token = create_test_jwt(&signing_key, &kid, wrong_jti.to_string());

	// Setup mock JWKS
	let jwks_response = serde_json::json!({
		"keys": [jwk]
	});

	Mock::given(method("GET"))
		.and(path("/.well-known/jwks.json"))
		.respond_with(ResponseTemplate::new(200).set_body_json(&jwks_response))
		.mount(&mock_server)
		.await;

	// Initialize JwksCache
	let redis = create_test_redis().await;
	let jwks_url = format!("{}/.well-known/jwks.json", mock_server.uri());
	let jwks_cache = Arc::new(JwksCache::new(jwks_url, Duration::from_secs(60), redis));

	// Create config for testing (production mode to enforce attestation)
	let config = create_test_config(Environment::Production);

	// Create test router with middleware
	let app = Router::new()
		.route("/test", post(|| async { StatusCode::OK }))
		.route_layer(axum::middleware::from_fn(
			|Extension(cfg): Extension<Arc<Config>>,
			 Extension(cache): Extension<Arc<JwksCache>>,
			 headers,
			 request,
			 next| async move { attestation_middleware(cfg, cache, headers, request, next).await },
		))
		.layer(Extension(config.clone()))
		.layer(Extension(jwks_cache.clone()));

	// Create multipart request body
	let body_data = create_multipart_body(metadata, boundary);

	let request = Request::builder()
		.method(Method::POST)
		.uri("/test")
		.header(
			header::CONTENT_TYPE,
			format!("multipart/form-data; boundary={}", boundary),
		)
		.header("attestation-gateway-token", &token)
		.body(Body::from(body_data))
		.unwrap();

	let response = app.oneshot(request).await.unwrap();

	// Assert failure due to hash mismatch (should return UNAUTHORIZED)
	assert_eq!(
		response.status(),
		StatusCode::UNAUTHORIZED,
		"Should fail with unauthorized for hash mismatch"
	);
}

#[tokio::test]
async fn test_skip_attestation_header_in_dev_mode() {
	// Setup config with development environment
	let config = create_test_config(Environment::Development);

	// Create a minimal JwksCache (won't be used when skipping)
	let redis = create_test_redis().await;
	let jwks_cache = Arc::new(JwksCache::new(
		"http://unused.example.com".to_string(),
		Duration::from_secs(60),
		redis,
	));

	// Create test router with middleware
	let app = Router::new()
		.route("/test", post(|| async { StatusCode::OK }))
		.route_layer(axum::middleware::from_fn(
			|Extension(cfg): Extension<Arc<Config>>,
			 Extension(cache): Extension<Arc<JwksCache>>,
			 headers,
			 request,
			 next| async move { attestation_middleware(cfg, cache, headers, request, next).await },
		))
		.layer(Extension(config.clone()))
		.layer(Extension(jwks_cache.clone()));

	// Create request WITHOUT attestation token but WITH skip header
	let request = Request::builder()
		.method(Method::POST)
		.uri("/test")
		.header(header::CONTENT_TYPE, "multipart/form-data; boundary=test")
		.header("x-e2e-skip-attestation", "true")
		.body(Body::from(
			"--test\r\nContent-Disposition: form-data; name=\"test\"\r\n\r\ndata\r\n--test--\r\n",
		))
		.unwrap();

	let response = app.oneshot(request).await.unwrap();

	// Should succeed without attestation verification
	assert_eq!(
		response.status(),
		StatusCode::OK,
		"Should skip attestation in dev mode with skip header"
	);
}

#[tokio::test]
async fn test_skip_attestation_blocked_in_production() {
	// Setup config with production environment
	let config = create_test_config(Environment::Production);

	let redis = create_test_redis().await;
	let jwks_cache = Arc::new(JwksCache::new(
		"http://unused.example.com".to_string(),
		Duration::from_secs(60),
		redis,
	));

	let app = Router::new()
		.route("/test", post(|| async { StatusCode::OK }))
		.route_layer(axum::middleware::from_fn(
			|Extension(cfg): Extension<Arc<Config>>,
			 Extension(cache): Extension<Arc<JwksCache>>,
			 headers,
			 request,
			 next| async move { attestation_middleware(cfg, cache, headers, request, next).await },
		))
		.layer(Extension(config.clone()))
		.layer(Extension(jwks_cache.clone()));

	// Try to skip attestation in production (should fail)
	let request = Request::builder()
		.method(Method::POST)
		.uri("/test")
		.header(header::CONTENT_TYPE, "multipart/form-data; boundary=test")
		.header("x-e2e-skip-attestation", "true")
		.body(Body::from(
			"--test\r\nContent-Disposition: form-data; name=\"test\"\r\n\r\ndata\r\n--test--\r\n",
		))
		.unwrap();

	let response = app.oneshot(request).await.unwrap();

	// Should fail because attestation token is missing (skip not allowed in prod)
	assert_eq!(
		response.status(),
		StatusCode::UNAUTHORIZED,
		"Should not skip attestation in production even with skip header"
	);
}
