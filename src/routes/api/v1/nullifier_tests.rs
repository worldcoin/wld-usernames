//! Integration tests use TEST_DATABASE_URL / TEST_REDIS_URL and a local mock verifier.
use super::{
	delete_profile_picture::delete_profile_picture, register_username::register_username,
	rename::rename, update_record::update_record,
};
use crate::{
	blocklist::Blocklist,
	config::{Config, Db, Environment},
	nullifier::canonical_nullifier,
};
use axum::{
	body::Body,
	http::{Request, StatusCode},
	routing::{delete, post},
	Extension, Router,
};
use serde_json::{json, Value};
use futures::FutureExt;
use sqlx::PgPool;
use std::{future::Future, panic::AssertUnwindSafe, sync::Arc};
use tokio::sync::OnceCell;
use tower::ServiceExt;
use wiremock::{
	matchers::{method, path},
	Mock, MockServer, ResponseTemplate,
};

async fn fixture(pool: &PgPool, verifier_status: u16) -> (Router, MockServer) {
	let _ = rustls::crypto::ring::default_provider().install_default();
	let verifier = MockServer::start().await;
	Mock::given(method("POST"))
		.and(path("/api/v2/verify/app_test_app_id"))
		.respond_with(ResponseTemplate::new(verifier_status).set_body_json(json!({
			"code": "invalid_proof", "detail": "Invalid proof", "attribute": null
		})))
		.mount(&verifier)
		.await;
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
		.layer(Extension(Db {
			read_only: pool.clone(),
			read_write: pool.clone(),
		}))
		.layer(Extension(Arc::new(blocklist)));
	(app, verifier)
}

fn registration(username: &str, nullifier: &str, address: &str) -> Value {
	json!({
		"username": username,
		"address": address,
		"nullifier_hash": nullifier,
		"verification_level": "orb",
		"proof": "mock-proof-bound-to-request-signal",
		"merkle_root": "0x01"
	})
}

async fn send(app: Router, path: &str, body: Value) -> StatusCode {
	app.oneshot(
		Request::post(path)
			.header("content-type", "application/json")
			.body(Body::from(body.to_string()))
			.unwrap(),
	)
	.await
	.unwrap()
	.status()
}

// Only migration completion is shared: each Tokio test owns its pool/runtime.
static MIGRATED: OnceCell<()> = OnceCell::const_new();

#[derive(Clone)]
struct TestData {
	pool: PgPool,
	id: String,
	address: String,
}

impl TestData {
	fn name(&self, suffix: &str) -> String {
		format!("n{}{suffix}", &self.id[..10])
	}

	fn nullifier(&self, suffix: &str) -> String {
		format!("{}{suffix}", self.id)
	}

	fn registration(&self, name: &str, nullifier: &str) -> Value {
		registration(&self.name(name), nullifier, &self.address)
	}

	async fn count(&self) -> i64 {
		sqlx::query_scalar("SELECT count(*) FROM names WHERE address = $1")
			.bind(&self.address)
			.fetch_one(&self.pool)
			.await
			.unwrap()
	}

	async fn seed_legacy(&self) -> String {
		let raw = format!("0X000{}", self.nullifier("a").to_uppercase());
		sqlx::query("INSERT INTO names (username,address,nullifier_hash,verification_level) VALUES ($1,$2,$3,'orb')")
			.bind(self.name("a")).bind(&self.address).bind(&raw)
			.execute(&self.pool).await.unwrap();
		raw
	}

	async fn stored(&self) -> String {
		sqlx::query_scalar("SELECT nullifier_hash FROM names WHERE username = $1")
			.bind(self.name("a"))
			.fetch_one(&self.pool)
			.await
			.unwrap()
	}
}

async fn with_database<F, Fut>(test: F)
where
	F: FnOnce(TestData) -> Fut,
	Fut: Future<Output = ()>,
{
	let url = std::env::var("TEST_DATABASE_URL")
		.expect("Set TEST_DATABASE_URL to the shared integration-test database");
	let pool = PgPool::connect(&url).await.unwrap();
	MIGRATED
		.get_or_init(|| async { sqlx::migrate!().run(&pool).await.unwrap() })
		.await;
	let id = uuid::Uuid::new_v4().simple().to_string();
	let data = TestData {
		pool,
		address: format!("0x00000000{id}")
			.parse::<alloy::primitives::Address>()
			.unwrap()
			.to_checksum(None),
		id,
	};
	let result = AssertUnwindSafe(test(data.clone())).catch_unwind().await;
	// Clean only this fixture's records, including after an assertion fails.
	sqlx::query("DELETE FROM old_names WHERE new_username IN (SELECT username FROM names WHERE address = $1)")
		.bind(&data.address).execute(&data.pool).await.unwrap();
	sqlx::query("DELETE FROM names WHERE address = $1")
		.bind(&data.address)
		.execute(&data.pool)
		.await
		.unwrap();
	data.pool.close().await;
	if let Err(panic) = result {
		std::panic::resume_unwind(panic);
	}
}

#[tokio::test]
async fn equivalent_nullifiers_cannot_register_twice() {
	with_database(|test| async move {
		let (app, verifier) = fixture(&test.pool, 200).await;
		let nullifier = test.nullifier("a");
		let raw = format!("0X000{}", nullifier.to_uppercase());
		assert_eq!(
			send(app.clone(), "/register", test.registration("a", &raw)).await,
			StatusCode::CREATED
		);
		for variant in [
			format!("0x{nullifier}"),
			nullifier.clone(),
			format!("000{nullifier}"),
			raw,
		] {
			assert_eq!(
				send(app.clone(), "/register", test.registration("b", &variant)).await,
				StatusCode::UNPROCESSABLE_ENTITY
			);
		}
		assert_eq!(
			test.stored().await,
			canonical_nullifier(&nullifier).unwrap().to_string()
		);
		assert_eq!(
			send(app, "/register", test.registration("c", &test.nullifier("b"))).await,
			StatusCode::CREATED
		);
		for request in verifier.received_requests().await.unwrap() {
			let body: Value = request.body_json().unwrap();
			let value = body["nullifier_hash"].as_str().unwrap();
			assert_eq!(canonical_nullifier(value).unwrap().to_string(), value);
		}
	})
	.await;
}

#[tokio::test]
async fn invalid_nullifier_or_proof_never_writes() {
	with_database(|test| async move {
		let (app, verifier) = fixture(&test.pool, 400).await;
		assert_eq!(
			send(app.clone(), "/register", test.registration("a", "0xg")).await,
			StatusCode::UNPROCESSABLE_ENTITY
		);
		assert!(verifier.received_requests().await.unwrap().is_empty());
		assert_eq!(
			send(app, "/register", test.registration("a", &test.nullifier("a"))).await,
			StatusCode::UNPROCESSABLE_ENTITY
		);
		assert_eq!(test.count().await, 0);
	})
	.await;
}

#[tokio::test]
async fn concurrent_registrations_return_one_conflict() {
	with_database(|test| async move {
		let (app, _verifier) = fixture(&test.pool, 200).await;
		let nullifier = test.nullifier("a");
		let (first, second) = tokio::join!(
			send(app.clone(), "/register", test.registration("a", &format!("0X0{}", nullifier.to_uppercase()))),
			send(app, "/register", test.registration("b", &nullifier)),
		);
		assert!(matches!(
			(first, second),
			(StatusCode::CREATED, StatusCode::UNPROCESSABLE_ENTITY)
				| (StatusCode::UNPROCESSABLE_ENTITY, StatusCode::CREATED)
		));
		assert_eq!(test.count().await, 1);
	})
	.await;
}

#[tokio::test]
async fn legacy_encoding_stays_owned_and_reserved() {
	with_database(|test| async move {
		let raw = test.seed_legacy().await;
		let (app, _verifier) = fixture(&test.pool, 200).await;
		let nullifier = test.nullifier("a");
		assert_eq!(
			send(app.clone(), "/register", test.registration("b", &nullifier)).await,
			StatusCode::UNPROCESSABLE_ENTITY
		);
		let mut update = test.registration("a", &nullifier);
		update.as_object_mut().unwrap().remove("username");
		let path = format!("/{}", test.name("a"));
		assert_eq!(send(app.clone(), &path, update.clone()).await, StatusCode::OK);
		update["nullifier_hash"] = json!(test.nullifier("b"));
		assert_eq!(send(app, &path, update).await, StatusCode::UNAUTHORIZED);
		assert_eq!(test.stored().await, raw);
	})
	.await;
}

#[tokio::test]
async fn legacy_rename_and_delete_compare_numeric_owners() {
	with_database(|test| async move {
		let redis_url = std::env::var("TEST_REDIS_URL")
			.expect("Set TEST_REDIS_URL to integration-test Redis");
		let redis = redis::aio::ConnectionManager::new(redis::Client::open(redis_url).unwrap())
			.await.unwrap();
		let (app, _verifier) = fixture(&test.pool, 200).await;
		let app = app.layer(Extension(redis));
		test.seed_legacy().await;
		let mut rename_body = json!({"old_username":test.name("a"),"new_username":test.name("b"),
			"nullifier_hash":test.nullifier("b"),"verification_level":"orb",
			"proof":"mock-proof","merkle_root":"0x01"});
		assert_eq!(send(app.clone(), "/rename", rename_body.clone()).await, StatusCode::UNAUTHORIZED);
		rename_body["nullifier_hash"] = json!(test.nullifier("a"));
		assert_eq!(send(app.clone(), "/rename", rename_body).await, StatusCode::OK);
		let query = format!("/profile-picture?address={}&nullifier_hash={}&verification_level=orb&proof=mock-proof&merkle_root=0x01", test.address, test.nullifier("b"));
		let response = app.clone().oneshot(Request::delete(&query).body(Body::empty()).unwrap()).await.unwrap();
		assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
		// Already-default picture tests authentication without contacting S3.
		std::env::set_var("PROFILE_PICTURE_CDN_URL", "https://cdn.example.invalid");
		std::env::set_var("MARBLE_CDN_URL", "https://marble.example.invalid");
		sqlx::query("UPDATE names SET profile_picture_url=$1 WHERE username=$2")
			.bind(format!("https://marble.example.invalid/{}.png", test.address.to_lowercase()))
			.bind(test.name("b")).execute(&test.pool).await.unwrap();
		let query = query.replace(&test.nullifier("b"), &format!("000{}", test.nullifier("a")));
		let response = app.oneshot(Request::delete(query).body(Body::empty()).unwrap()).await.unwrap();
		assert_eq!(response.status(), StatusCode::NO_CONTENT);
	})
	.await;
}

#[tokio::test]
async fn database_enforces_binary_identity_for_legacy_writers() {
	with_database(|test| async move {
		let nullifier = test.nullifier("a");
		let raw = format!("0X000{}", nullifier.to_uppercase());
		let insert = |name: String, value: String| {
			let pool = test.pool.clone();
			let address = test.address.clone();
			async move {
				sqlx::query("INSERT INTO names (username,address,nullifier_hash,verification_level) VALUES ($1,$2,$3,'orb')")
					.bind(name).bind(address).bind(value).execute(&pool).await
			}
		};
		let (first, second) = tokio::join!(
			insert(test.name("a"), raw),
			insert(test.name("b"), nullifier.clone())
		);
		assert_ne!(first.is_ok(), second.is_ok());
		let error = first.err().or_else(|| second.err()).unwrap();
		assert_eq!(
			error.as_database_error().unwrap().constraint(),
			Some("names_nullifier_canonical_unique")
		);
		assert_eq!(test.count().await, 1);
		insert(test.name("c"), test.nullifier("b")).await.unwrap();
		let error = sqlx::query("UPDATE names SET nullifier_hash=$1 WHERE username=$2")
			.bind(&nullifier).bind(test.name("c")).execute(&test.pool).await.unwrap_err();
		assert_eq!(
			error.as_database_error().unwrap().constraint(),
			Some("names_nullifier_canonical_unique")
		);
		let same_encoding = test.nullifier("c");
		let (first, second) = tokio::join!(
			insert(test.name("e"), same_encoding.clone()),
			insert(test.name("f"), same_encoding),
		);
		assert_ne!(first.is_ok(), second.is_ok());
		let error = first.err().or_else(|| second.err()).unwrap();
		assert_eq!(
			error.as_database_error().unwrap().constraint(),
			Some("names_nullifier_canonical_unique")
		);
		assert_eq!(test.count().await, 3);
		for invalid in [
			"", "0x", "0X", "-1", "+1", " 1", "1 ", "1\n", "\t1", "0xg", "0x0x1", "１", "v4:0xa",
			"30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000001",
			&"0".repeat(65), &"f".repeat(64),
		] {
			let error = insert(test.name("d"), invalid.to_string()).await.unwrap_err();
			assert_eq!(
				error.as_database_error().unwrap().code().as_deref(),
				Some("22023")
			);
		}
		for value in [
			"0",
			"100",
			"30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000000",
		] {
			let bytes: Vec<u8> = sqlx::query_scalar("SELECT canonical_world_id_nullifier($1)")
				.bind(value).fetch_one(&test.pool).await.unwrap();
			assert_eq!(bytes.len(), 32);
			assert_eq!(bytes, canonical_nullifier(value).unwrap().as_bytes());
		}
	})
	.await;
}
