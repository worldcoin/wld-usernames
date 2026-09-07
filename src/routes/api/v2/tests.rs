use super::*;
use alloy::signers::local::PrivateKeySigner;
use sqlx::PgPool;
use std::{str::FromStr, sync::Arc};
use wiremock::{
	matchers::{body_partial_json, method, path},
	Mock, MockServer, ResponseTemplate,
};

fn pending(operation: &str, username: &str, session: Option<&str>) -> Challenge {
	let now = chrono::Utc::now().timestamp();
	Challenge {
		id: Uuid::new_v4().to_string(),
		operation: operation.into(),
		username: username.into(),
		new_username: None,
		address: format!("0x{}", "11".repeat(20)),
		previous_address: None,
		nonce: proof::hash(Uuid::new_v4().as_bytes()),
		session_id: session.map(Into::into),
		signal: "bound-operation".into(),
		created_at: now,
		expires_at: now + TTL,
		consumed: false,
	}
}

fn session(commitment: u8) -> String {
	format!("session_{commitment:064x}01{}", "00".repeat(31))
}

fn payload(challenge: &Challenge, session: &str, owner: Option<&str>) -> Submit {
	Submit {
		challenge_id: challenge.id.clone(),
		proof: proof::Proof {
			session_id: session.into(),
			response: proof::Response {
				proof: std::array::from_fn(|_| "0x1".into()),
				expires_at_min: u64::try_from(challenge.created_at).unwrap(),
				nullifier: owner.map(Into::into),
				session_nullifier: if owner.is_none() {
					Some(["0x1".into(), "0x2".into()])
				} else {
					None
				},
			},
		},
		legacy_proof: None,
	}
}

async fn insert_challenge(pool: &PgPool, c: &Challenge) {
	sqlx::query("INSERT INTO username_v4_challenges
        (id,operation,username,new_username,address,previous_address,nonce,session_id,signal,created_at,expires_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(&c.id).bind(&c.operation).bind(&c.username).bind(&c.new_username)
        .bind(&c.address).bind(&c.previous_address).bind(&c.nonce).bind(&c.session_id)
        .bind(&c.signal).bind(c.created_at).bind(c.expires_at).execute(pool).await.unwrap();
}

async fn environment() -> (MockServer, Arc<crate::config::Config>, ConnectionManager) {
	let server = MockServer::start().await;
	let mut config = crate::config::Config::test_config(crate::config::Environment::Development);
	config.developer_portal_url = server.uri();
	config.world_id_v4 = Some(proof::Settings {
		rp_id: "rp_0000000000000001".into(),
		signer: PrivateKeySigner::from_str(&format!("{:064x}", 1)).unwrap(),
	});
	let redis = redis::Client::open(std::env::var("TEST_REDIS_URL").unwrap())
		.unwrap()
		.get_connection_manager()
		.await
		.unwrap();
	(server, Arc::new(config), redis)
}

async fn accept(server: &MockServer, c: &Challenge, session: &str, owner: Option<&str>) {
	let mut response = json!({
		"success": true, "protocol_version": "4.0", "session_id": session,
		"environment": "staging",
	});
	if let Some(owner) = owner {
		response["action"] = json!(proof::OWNER_ACTION);
		response["nullifier"] = json!(proof::field(owner).unwrap());
	}
	Mock::given(method("POST"))
		.and(path("/api/v4/verify/rp_0000000000000001"))
		.and(body_partial_json(json!({
			"nonce": c.nonce, "session_id": session, "min_protocol_version": "4.0",
			"responses": [{ "identifier": "orb", "issuer_schema_id": 1,
				"signal_hash": proof::hash(c.signal.as_bytes()) }],
		})))
		.respond_with(ResponseTemplate::new(200).set_body_json(response))
		.mount(server)
		.await;
}

async fn submit(
	pool: &PgPool,
	config: Arc<crate::config::Config>,
	redis: ConnectionManager,
	payload: Submit,
) -> Result<StatusCode, ErrorResponse> {
	complete(
		Extension(config),
		Extension(Db {
			read_only: pool.clone(),
			read_write: pool.clone(),
		}),
		Extension(redis),
		Json(payload),
	)
	.await
}

#[test]
fn expiry_and_wire_request_bindings() {
	let c = pending("register", "alice", None);
	assert!(ensure_live(&c, c.expires_at - 1).is_ok());
	assert!(ensure_live(&c, c.expires_at).is_err());
	assert!(ensure_live(&c, c.created_at - 1).is_err());
	let settings = proof::Settings {
		rp_id: "rp_0000000000000001".into(),
		signer: PrivateKeySigner::from_str(&format!("{:064x}", 1)).unwrap(),
	};
	let request = settings.request(&c).unwrap();
	assert_eq!(request["version"], 1);
	assert_eq!(request["session_id"], "create");
	assert_eq!(request["proof_type"], "uniqueness");
	assert_eq!(
		request["action"],
		proof::hash(proof::OWNER_ACTION.as_bytes())
	);
	assert_eq!(
		request["proof_requests"][0]["signal"],
		format!("0x{}", hex::encode(c.signal.as_bytes()))
	);
	let mut resumed = c;
	resumed.session_id = Some(session(1));
	let request = settings.request(&resumed).unwrap();
	assert_eq!(request["session_id"], session(1));
	assert_eq!(request["proof_type"], "session");
	assert!(request["action"].is_null());
}

#[test]
fn canonical_fields_and_session_validation() {
	assert_eq!(proof::field("A").unwrap(), proof::field("0x000a").unwrap());
	assert!(proof::field("0x").is_err());
	assert!(proof::field(&"f".repeat(64)).is_err());
	assert!(proof::valid_session(&session(1)));
	assert!(!proof::valid_session(&session(0)));
	assert!(!proof::valid_session("session_invalid"));
	assert!(!proof::valid_session(&session(1).replace("01", "00")));
}

#[test]
fn rp_signature_matches_the_official_idkit_server_fixture() {
	let mut c = pending("register", "alice", None);
	c.nonce = "0x000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f".into();
	c.created_at = 1_700_000_000;
	c.expires_at = 1_700_000_300;
	let settings = proof::Settings {
		rp_id: "rp_0000000000000001".into(),
		signer: PrivateKeySigner::from_str(&format!("{:064x}", 1)).unwrap(),
	};
	// Generated independently with idkit-server computeRpSignatureMessage and
	// ethers Wallet.signMessage using the public test key 1.
	assert_eq!(settings.request(&c).unwrap()["signature"],
        "0xbabbe131da2ec7ee224173303fa6e3b246d321e96ac52aba65596df2e93f46130ea30706e2ee55c616a75b37e2962a4277fe6df956695cfbf3d455e8e0d89ca11b");
}

#[sqlx::test]
#[ignore = "requires PostgreSQL and TEST_REDIS_URL; run in the recovery integration CI job"]
async fn recovery_rename_replay_and_legacy_write_protection(pool: PgPool) {
	let (server, config, redis) = environment().await;
	let registration = pending("register", "alice", None);
	insert_challenge(&pool, &registration).await;
	accept(&server, &registration, &session(1), Some("0x1")).await;
	assert_eq!(
		submit(
			&pool,
			config.clone(),
			redis.clone(),
			payload(&registration, &session(1), Some("0x1"))
		)
		.await
		.unwrap(),
		StatusCode::CREATED
	);
	// Retrying a committed operation is idempotent.
	assert_eq!(
		submit(
			&pool,
			config.clone(),
			redis.clone(),
			payload(&registration, &session(1), Some("0x1"))
		)
		.await
		.unwrap(),
		StatusCode::OK
	);

	let mut recovery = pending("recover", "alice", Some(&session(1)));
	recovery.previous_address = Some(registration.address.clone());
	recovery.address = format!("0x{}", "22".repeat(20));
	insert_challenge(&pool, &recovery).await;
	accept(&server, &recovery, &session(1), None).await;
	// A valid proof for a different session cannot take ownership.
	assert!(submit(
		&pool,
		config.clone(),
		redis.clone(),
		payload(&recovery, &session(2), None)
	)
	.await
	.is_err());
	submit(
		&pool,
		config.clone(),
		redis.clone(),
		payload(&recovery, &session(1), None),
	)
	.await
	.unwrap();
	assert!(submit(
		&pool,
		config.clone(),
		redis.clone(),
		payload(&registration, &session(1), Some("0x1"))
	)
	.await
	.is_err());
	let address: String = sqlx::query_scalar("SELECT address FROM names WHERE username='alice'")
		.fetch_one(&pool)
		.await
		.unwrap();
	assert_eq!(address, recovery.address);
	// Even a legacy write which bypasses application checks is rejected in SQL.
	assert!(
		sqlx::query("UPDATE names SET address='stale' WHERE username='alice'")
			.execute(&pool)
			.await
			.is_err()
	);

	let mut rename = pending("rename", "alice", Some(&session(1)));
	rename.address = recovery.address.clone();
	rename.previous_address = Some(recovery.address.clone());
	rename.new_username = Some("newalice".into());
	insert_challenge(&pool, &rename).await;
	accept(&server, &rename, &session(1), None).await;
	submit(&pool, config, redis, payload(&rename, &session(1), None))
		.await
		.unwrap();
	let name: String = sqlx::query_scalar("SELECT username FROM username_v4_owners")
		.fetch_one(&pool)
		.await
		.unwrap();
	assert_eq!(name, "newalice");
	assert!(sqlx::query("INSERT INTO names(username,address,nullifier_hash,verification_level) VALUES ('ALICE','other','other','orb')").execute(&pool).await.is_err());
}

#[sqlx::test]
#[ignore = "requires PostgreSQL and TEST_REDIS_URL; run in the recovery integration CI job"]
async fn concurrent_double_claim_rolls_back_loser(pool: PgPool) {
	let (server, config, redis) = environment().await;
	let a = pending("register", "alice", None);
	let mut b = pending("register", "bob", None);
	b.address = format!("0x{}", "33".repeat(20));
	insert_challenge(&pool, &a).await;
	insert_challenge(&pool, &b).await;
	accept(&server, &a, &session(1), Some("0x1")).await;
	accept(&server, &b, &session(2), Some("0x0001")).await;
	let (a, b) = tokio::join!(
		submit(
			&pool,
			config.clone(),
			redis.clone(),
			payload(&a, &session(1), Some("0x1"))
		),
		submit(
			&pool,
			config,
			redis,
			payload(&b, &session(2), Some("0x0001"))
		)
	);
	assert_ne!(a.is_ok(), b.is_ok());
	let counts: (i64, i64, i64) = sqlx::query_as(
        "SELECT (SELECT count(*) FROM names),(SELECT count(*) FROM username_v4_owners),(SELECT count(*) FROM username_v4_challenges WHERE consumed)")
        .fetch_one(&pool).await.unwrap();
	assert_eq!(counts, (1, 1, 1));
}

#[sqlx::test]
#[ignore = "requires PostgreSQL and TEST_REDIS_URL; run in the recovery integration CI job"]
async fn migration_requires_legacy_owner_and_preserves_legacy_uniqueness(pool: PgPool) {
	let (server, config, redis) = environment().await;
	let mut c = pending("migrate", "alice", None);
	c.previous_address = Some(c.address.clone());
	sqlx::query("INSERT INTO names(username,address,nullifier_hash,verification_level) VALUES ('alice',$1,$2,'orb')")
        .bind(&c.address).bind(proof::field("0x1").unwrap()).execute(&pool).await.unwrap();
	insert_challenge(&pool, &c).await;
	accept(&server, &c, &session(1), Some("0x3")).await;
	Mock::given(method("POST"))
		.and(path("/api/v2/verify/app_test_app_id"))
		.respond_with(ResponseTemplate::new(200))
		.mount(&server)
		.await;
	let mut wrong = payload(&c, &session(1), Some("0x3"));
	wrong.legacy_proof = Some(LegacyProof {
		merkle_root: "0x1".into(),
		nullifier_hash: "0x2".into(),
		proof: "0x1".into(),
	});
	assert!(submit(&pool, config.clone(), redis.clone(), wrong)
		.await
		.is_err());
	let mut correct = payload(&c, &session(1), Some("0x3"));
	correct.legacy_proof = Some(LegacyProof {
		merkle_root: "0x1".into(),
		nullifier_hash: "0x1".into(),
		proof: "0x1".into(),
	});
	submit(&pool, config, redis, correct).await.unwrap();
	let legacy: String =
		sqlx::query_scalar("SELECT nullifier_hash FROM names WHERE username='alice'")
			.fetch_one(&pool)
			.await
			.unwrap();
	assert_eq!(legacy, proof::field("0x1").unwrap());
}
