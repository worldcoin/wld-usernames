//! These tests use only disposable PostgreSQL and a local mock verifier.
use super::{
	register_username::register_username, update_record::update_record,
	rename::rename, delete_profile_picture::delete_profile_picture,
};
use crate::{
	blocklist::Blocklist,
	config::{Config, Db, Environment},
	nullifier::canonical_nullifier,
};
use axum::{body::Body, http::{Request, StatusCode}, routing::{post, delete}, Extension, Router};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tower::ServiceExt;
use wiremock::{matchers::{method, path}, Mock, MockServer, ResponseTemplate};

async fn fixture(pool: &PgPool, verifier_status: u16) -> (Router, MockServer) {
	let _ = rustls::crypto::ring::default_provider().install_default();
	let verifier = MockServer::start().await;
	Mock::given(method("POST"))
		.and(path("/api/v2/verify/app_test_app_id"))
		.respond_with(ResponseTemplate::new(verifier_status).set_body_json(json!({
			"code": "invalid_proof", "detail": "Invalid proof", "attribute": null
		})))
		.mount(&verifier).await;
	let mut config = Config::test_config(Environment::Development);
	config.developer_portal_url = verifier.uri();
	// Use a nonmatching substring: an empty substring blocks every name.
	let blocklist = Blocklist::new("reserved", "blockedword");
	let app = Router::new()
		.route("/register", post(register_username))
		.route("/:name", post(update_record))
		.route("/rename", post(rename))
		.route("/profile-picture", delete(delete_profile_picture))
		.layer(Extension(Arc::new(config)))
		.layer(Extension(Db { read_only: pool.clone(), read_write: pool.clone() }))
		.layer(Extension(Arc::new(blocklist)));
	(app, verifier)
}

fn registration(username: &str, nullifier: &str) -> Value {
	json!({
		"username": username,
		"address": "0x0000000000000000000000000000000000000001",
		"nullifier_hash": nullifier,
		"verification_level": "orb",
		"proof": "mock-proof-bound-to-request-signal",
		"merkle_root": "0x01"
	})
}

async fn send(app: Router, path: &str, body: Value) -> StatusCode {
	app.oneshot(Request::post(path).header("content-type", "application/json")
		.body(Body::from(body.to_string())).unwrap()).await.unwrap().status()
}

#[sqlx::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn equivalent_nullifiers_cannot_register_twice(pool: PgPool) {
	let (app, verifier) = fixture(&pool, 200).await;
	assert_eq!(send(app.clone(), "/register", registration("alice", "0X000A")).await, StatusCode::CREATED);
	for variant in ["0xa", "a", "000a", "0x0A"] {
		assert_eq!(send(app.clone(), "/register", registration("bobby", variant)).await, StatusCode::UNPROCESSABLE_ENTITY);
	}
	let stored: String = sqlx::query_scalar("SELECT nullifier_hash FROM names WHERE username='alice'")
		.fetch_one(&pool).await.unwrap();
	assert_eq!(stored, canonical_nullifier("a").unwrap());
	assert_eq!(send(app, "/register", registration("carol", "b")).await, StatusCode::CREATED);
	let requests = verifier.received_requests().await.unwrap();
	for request in requests {
		let body: Value = request.body_json().unwrap();
		let value = body["nullifier_hash"].as_str().unwrap();
		assert_eq!(canonical_nullifier(value).unwrap(), value);
	}
}

#[sqlx::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn invalid_nullifier_or_proof_never_writes(pool: PgPool) {
	let (app, verifier) = fixture(&pool, 400).await;
	assert_eq!(send(app.clone(), "/register", registration("alice", "0xg")).await, StatusCode::UNPROCESSABLE_ENTITY);
	assert!(verifier.received_requests().await.unwrap().is_empty());
	assert_eq!(send(app, "/register", registration("alice", "a")).await, StatusCode::UNPROCESSABLE_ENTITY);
	let count: i64 = sqlx::query_scalar("SELECT count(*) FROM names").fetch_one(&pool).await.unwrap();
	assert_eq!(count, 0);
}

#[sqlx::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn concurrent_registrations_return_one_conflict(pool: PgPool) {
	let (app, _verifier) = fixture(&pool, 200).await;
	let (first, second) = tokio::join!(
		send(app.clone(), "/register", registration("alice", "0x0A")),
		send(app, "/register", registration("bobby", "a")),
	);
	assert!(matches!((first, second),
		(StatusCode::CREATED, StatusCode::UNPROCESSABLE_ENTITY) |
		(StatusCode::UNPROCESSABLE_ENTITY, StatusCode::CREATED)));
	let count: i64 = sqlx::query_scalar("SELECT count(*) FROM names").fetch_one(&pool).await.unwrap();
	assert_eq!(count, 1);
}

#[sqlx::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL"]
async fn legacy_encoding_stays_owned_and_reserved(pool: PgPool) {
	sqlx::query("INSERT INTO names (username,address,nullifier_hash,verification_level) VALUES ('alice',$1,'0X000A','orb')")
		.bind("0x0000000000000000000000000000000000000001").execute(&pool).await.unwrap();
	let (app, _verifier) = fixture(&pool, 200).await;
	assert_eq!(send(app.clone(), "/register", registration("bobby", "a")).await, StatusCode::UNPROCESSABLE_ENTITY);
	let mut update = registration("alice", "a");
	update.as_object_mut().unwrap().remove("username");
	assert_eq!(send(app.clone(), "/alice", update.clone()).await, StatusCode::OK);
	update["nullifier_hash"] = json!("b");
	assert_eq!(send(app, "/alice", update).await, StatusCode::UNAUTHORIZED);
	let stored: String = sqlx::query_scalar("SELECT nullifier_hash FROM names WHERE username='alice'")
		.fetch_one(&pool).await.unwrap();
	assert_eq!(stored, "0X000A");
}

#[sqlx::test]
#[ignore = "requires disposable PostgreSQL DATABASE_URL and TEST_REDIS_URL"]
async fn legacy_rename_and_delete_compare_numeric_owners(pool: PgPool) {
	let redis_url = std::env::var("TEST_REDIS_URL").expect("Set TEST_REDIS_URL to disposable Redis");
	let redis = redis::aio::ConnectionManager::new(redis::Client::open(redis_url).unwrap()).await.unwrap();
	let (app, _verifier) = fixture(&pool, 200).await;
	let app = app.layer(Extension(redis));
	sqlx::query("INSERT INTO names (username,address,nullifier_hash,verification_level) VALUES ('alice',$1,'0X000A','orb')")
		.bind("0x0000000000000000000000000000000000000001").execute(&pool).await.unwrap();
	let rename_body = json!({"old_username":"alice","new_username":"bobby","nullifier_hash":"b",
		"verification_level":"orb","proof":"mock-proof","merkle_root":"0x01"});
	assert_eq!(send(app.clone(), "/rename", rename_body.clone()).await, StatusCode::UNAUTHORIZED);
	let mut rename_body = rename_body;
	rename_body["nullifier_hash"] = json!("a");
	assert_eq!(send(app.clone(), "/rename", rename_body).await, StatusCode::OK);
	let query = "/profile-picture?address=0x0000000000000000000000000000000000000001&nullifier_hash=b&verification_level=orb&proof=mock-proof&merkle_root=0x01";
	let response = app.clone().oneshot(Request::delete(query).body(Body::empty()).unwrap()).await.unwrap();
	assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
	// Already-default picture avoids S3 and tests the authenticated no-op path.
	std::env::set_var("PROFILE_PICTURE_CDN_URL", "https://cdn.example.invalid");
	std::env::set_var("MARBLE_CDN_URL", "https://marble.example.invalid");
	sqlx::query("UPDATE names SET profile_picture_url=$1 WHERE username='bobby'")
		.bind("https://marble.example.invalid/0x0000000000000000000000000000000000000001.png")
		.execute(&pool).await.unwrap();
	let response = app.oneshot(Request::delete(query.replace("nullifier_hash=b", "nullifier_hash=000a"))
		.body(Body::empty()).unwrap()).await.unwrap();
	assert_eq!(response.status(), StatusCode::NO_CONTENT);
}
