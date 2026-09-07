//! Recovery-safe username ownership. Uniqueness establishes one owner; subsequent
//! changes prove the server-stored session, which survives authenticator recovery.
pub mod proof;
#[cfg(test)]
mod tests;

use crate::{
	blocklist::BlocklistExt,
	cache,
	config::{ConfigExt, Db, USERNAME_REGEX},
	types::{Address, ErrorResponse, Name},
	verify,
};
use aide::axum::ApiRouter;
use axum::{routing::post, Extension, Json};
use http::StatusCode;
use redis::aio::ConnectionManager;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{FromRow, PgConnection};
use uuid::Uuid;

const TTL: i64 = 300;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChallengeRequest {
	operation: String,
	username: String,
	new_username: Option<String>,
	address: Address,
}

#[derive(Clone, FromRow, Serialize)]
pub(super) struct Challenge {
	id: String,
	operation: String,
	username: String,
	new_username: Option<String>,
	address: String,
	previous_address: Option<String>,
	nonce: String,
	session_id: Option<String>,
	signal: String,
	created_at: i64,
	expires_at: i64,
	consumed: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyProof {
	merkle_root: String,
	nullifier_hash: String,
	proof: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Submit {
	challenge_id: String,
	proof: proof::Proof,
	legacy_proof: Option<LegacyProof>,
}

pub fn handler() -> ApiRouter {
	ApiRouter::new()
		.route("/challenge", post(challenge))
		.route("/complete", post(complete))
}

async fn challenge(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(blocklist): BlocklistExt,
	Json(input): Json<ChallengeRequest>,
) -> Result<Json<Value>, ErrorResponse> {
	let settings = config.world_id_v4.as_ref().ok_or_else(|| {
		ErrorResponse::service_unavailable("World ID 4 usernames are not configured".into())
	})?;
	if !matches!(
		input.operation.as_str(),
		"register" | "migrate" | "rename" | "recover"
	) || (input.operation == "rename") != input.new_username.is_some()
	{
		return Err(ErrorResponse::bad_request("Invalid username operation"));
	}
	for name in std::iter::once(&input.username).chain(input.new_username.iter()) {
		if !USERNAME_REGEX.is_match(name) {
			return Err(ErrorResponse::bad_request("Invalid username"));
		}
	}
	let target = input.new_username.as_ref().unwrap_or(&input.username);
	if matches!(input.operation.as_str(), "register" | "rename") {
		blocklist
			.ensure_valid(target)
			.map_err(|e| ErrorResponse::bad_request(e.to_string()))?;
	}
	let address = input.address.to_checksum(None);
	let record = sqlx::query_as::<_, Name>("SELECT * FROM names WHERE LOWER(username) = LOWER($1)")
		.bind(&input.username)
		.fetch_optional(&db.read_write)
		.await?;
	let mut username = input.username;
	let mut previous_address = None;
	let mut session_id = None;
	if input.operation == "register" {
		if record.is_some() {
			return Err(ErrorResponse::bad_request("Username is taken"));
		}
	} else {
		let record = record.ok_or_else(|| ErrorResponse::not_found("Username not found".into()))?;
		username = record.username;
		previous_address = Some(record.address);
		session_id = sqlx::query_scalar::<_, String>(
			"SELECT session_id FROM username_v4_owners WHERE username = $1",
		)
		.bind(&username)
		.fetch_optional(&db.read_write)
		.await?;
		if input.operation == "migrate" {
			if session_id.is_some() || record.nullifier_hash.starts_with("v4:") {
				return Err(ErrorResponse::bad_request("Username is already migrated"));
			}
		} else if session_id.is_none() {
			return Err(ErrorResponse::bad_request(
				"Username must be migrated with its legacy ownership proof first",
			));
		}
		if input.operation == "rename" && previous_address.as_ref() != Some(&address) {
			return Err(ErrorResponse::bad_request(
				"Recover the wallet address before renaming",
			));
		}
	}
	if input
		.new_username
		.as_ref()
		.is_some_and(|name| name.eq_ignore_ascii_case(&username))
	{
		return Err(ErrorResponse::bad_request("The new username must differ"));
	}
	let id = Uuid::new_v4().to_string();
	let nonce = proof::hash(Uuid::new_v4().as_bytes());
	let created_at = chrono::Utc::now().timestamp();
	// JSON array encoding avoids ambiguous concatenation of usernames and addresses.
	let signal = serde_json::to_string(&json!([
		"world_usernames:v2",
		id,
		input.operation,
		username,
		input.new_username,
		address,
		previous_address,
		nonce
	]))?;
	let challenge = Challenge {
		id,
		operation: input.operation,
		username,
		new_username: input.new_username,
		address,
		previous_address,
		nonce,
		session_id,
		signal,
		created_at,
		expires_at: created_at + TTL,
		consumed: false,
	};
	let request = settings.request(&challenge)?;
	sqlx::query(
        "INSERT INTO username_v4_challenges
        (id, operation, username, new_username, address, previous_address, nonce, session_id, signal, created_at, expires_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)")
        .bind(&challenge.id).bind(&challenge.operation).bind(&challenge.username)
        .bind(&challenge.new_username).bind(&challenge.address).bind(&challenge.previous_address)
        .bind(&challenge.nonce).bind(&challenge.session_id).bind(&challenge.signal)
        .bind(challenge.created_at).bind(challenge.expires_at).execute(&db.read_write).await?;
	Ok(Json(json!({
		"challenge_id": challenge.id,
		"proof_request": request,
		"legacy_signal": if challenge.operation == "migrate" { Some(&challenge.signal) } else { None },
		"expires_at": challenge.expires_at,
	})))
}

async fn complete(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Json(input): Json<Submit>,
) -> Result<StatusCode, ErrorResponse> {
	let challenge =
		sqlx::query_as::<_, Challenge>("SELECT * FROM username_v4_challenges WHERE id = $1")
			.bind(&input.challenge_id)
			.fetch_optional(&db.read_write)
			.await?
			.ok_or_else(|| ErrorResponse::not_found("Challenge not found".into()))?;
	if challenge.consumed {
		let mut tx = db.read_write.begin().await?;
		ensure_completed_state(&mut tx, &challenge, &input.proof.session_id).await?;
		return Ok(StatusCode::OK);
	}
	ensure_live(&challenge, chrono::Utc::now().timestamp())?;
	let owner = proof::verify(&config, &challenge, &input.proof).await?;
	let legacy_nullifier = if challenge.operation == "migrate" {
		let legacy = input
			.legacy_proof
			.as_ref()
			.ok_or_else(|| ErrorResponse::bad_request("Legacy ownership proof is required"))?;
		verify::dev_portal_verify_proof(
			idkit::Proof {
				merkle_root: legacy.merkle_root.clone(),
				nullifier_hash: legacy.nullifier_hash.clone(),
				proof: legacy.proof.clone(),
				verification_level: idkit::session::VerificationLevel::Orb,
			},
			config.wld_app_id.to_string(),
			"username",
			challenge.signal.as_str(),
			config.developer_portal_url.clone(),
		)
		.await
		.map_err(|_| ErrorResponse::unauthorized("Invalid legacy ownership proof".into()))?;
		Some(proof::field(&legacy.nullifier_hash)?)
	} else {
		if input.legacy_proof.is_some() {
			return Err(ErrorResponse::bad_request("Unexpected legacy proof"));
		}
		None
	};
	let mut tx = db.read_write.begin().await?;
	let locked = sqlx::query_as::<_, Challenge>(
		"SELECT * FROM username_v4_challenges WHERE id = $1 FOR UPDATE",
	)
	.bind(&challenge.id)
	.fetch_one(&mut *tx)
	.await?;
	if locked.consumed {
		ensure_completed_state(&mut tx, &locked, &input.proof.session_id).await?;
		return Ok(StatusCode::OK);
	}
	ensure_live(&locked, chrono::Utc::now().timestamp())?;
	// Serialize against legacy name/alias writers as well as other v4 claims.
	sqlx::query("LOCK TABLE names, old_names IN SHARE ROW EXCLUSIVE MODE")
		.execute(&mut *tx)
		.await?;
	sqlx::query("SELECT set_config('world_usernames.v4_write', 'on', true)")
		.execute(&mut *tx)
		.await?;
	let aliases: Vec<String> =
		sqlx::query_scalar("SELECT old_username FROM old_names WHERE new_username = $1")
			.bind(&locked.username)
			.fetch_all(&mut *tx)
			.await?;
	apply(
		&mut tx,
		&locked,
		&input.proof.session_id,
		owner.as_deref(),
		legacy_nullifier.as_deref(),
	)
	.await?;
	sqlx::query("UPDATE username_v4_challenges SET consumed = TRUE WHERE id = $1")
		.bind(&locked.id)
		.execute(&mut *tx)
		.await?;
	// Invalidate before commit: a stale read cannot resurrect the old wallet
	// mapping. Redis failure rolls back the ownership change so it can be retried.
	let keys = invalidation_keys(&locked).into_iter().chain(
		aliases
			.into_iter()
			.map(|alias| format!("query_single:{}", alias.to_lowercase())),
	);
	for key in keys {
		cache::write_tombstone(&mut redis, &key).await?;
	}
	tx.commit().await?;
	Ok(if locked.operation == "register" {
		StatusCode::CREATED
	} else {
		StatusCode::OK
	})
}

fn ensure_live(challenge: &Challenge, now: i64) -> Result<(), ErrorResponse> {
	if now >= challenge.expires_at || now < challenge.created_at {
		return Err(ErrorResponse::bad_request(
			"Challenge expired; request a new one",
		));
	}
	Ok(())
}

async fn ensure_completed_state(
	db: &mut PgConnection,
	challenge: &Challenge,
	session: &str,
) -> Result<(), ErrorResponse> {
	let current: bool = sqlx::query_scalar(
		"SELECT EXISTS(SELECT 1 FROM names n JOIN username_v4_owners o USING(username)
        WHERE n.username = $1 AND n.address = $2 AND o.session_id = $3)",
	)
	.bind(
		challenge
			.new_username
			.as_ref()
			.unwrap_or(&challenge.username),
	)
	.bind(&challenge.address)
	.bind(session)
	.fetch_one(db)
	.await?;
	if current {
		Ok(())
	} else {
		Err(ErrorResponse::bad_request(
			"Completed challenge has been superseded",
		))
	}
}

async fn apply(
	db: &mut PgConnection,
	challenge: &Challenge,
	session: &str,
	owner: Option<&str>,
	legacy_nullifier: Option<&str>,
) -> Result<(), ErrorResponse> {
	let target = challenge
		.new_username
		.as_ref()
		.unwrap_or(&challenge.username);
	if let Some(owner) = owner {
		let claimed: bool = sqlx::query_scalar(
			"SELECT EXISTS(SELECT 1 FROM username_v4_owners WHERE owner_nullifier = $1)",
		)
		.bind(owner)
		.fetch_one(&mut *db)
		.await?;
		if claimed {
			return Err(ErrorResponse::bad_request(
				"This World ID already owns a username; recover its existing session",
			));
		}
	}
	let occupied: bool = sqlx::query_scalar(
		"SELECT EXISTS(SELECT 1 FROM names WHERE address = $1 AND username != $2)",
	)
	.bind(&challenge.address)
	.bind(&challenge.username)
	.fetch_one(&mut *db)
	.await?;
	if occupied {
		return Err(ErrorResponse::bad_request("Wallet already has a username"));
	}
	if matches!(challenge.operation.as_str(), "register" | "rename") {
		let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM names WHERE LOWER(username) = LOWER($1)
            UNION SELECT 1 FROM old_names WHERE LOWER(old_username) = LOWER($1) AND new_username != $2)")
            .bind(target).bind(&challenge.username).fetch_one(&mut *db).await?;
		if taken {
			return Err(ErrorResponse::bad_request("Username is taken"));
		}
	}
	if challenge.operation == "register" {
		let owner = owner.ok_or_else(|| ErrorResponse::bad_request("Missing unique owner"))?;
		sqlx::query("INSERT INTO names (username,address,nullifier_hash,verification_level) VALUES ($1,$2,$3,'orb')")
            .bind(target).bind(&challenge.address).bind(format!("v4:{owner}")).execute(&mut *db).await?;
	} else {
		let record =
			sqlx::query_as::<_, Name>("SELECT * FROM names WHERE username = $1 FOR UPDATE")
				.bind(&challenge.username)
				.fetch_optional(&mut *db)
				.await?
				.ok_or_else(|| ErrorResponse::not_found("Username no longer exists".into()))?;
		if challenge.previous_address.as_ref() != Some(&record.address) {
			return Err(ErrorResponse::bad_request(
				"Ownership changed; request a new challenge",
			));
		}
		if challenge.operation == "migrate" {
			if legacy_nullifier != Some(proof::field(&record.nullifier_hash)?.as_str()) {
				return Err(ErrorResponse::unauthorized(
					"Legacy proof does not own this username".into(),
				));
			}
			sqlx::query(
				"UPDATE names SET address = $1, updated_at = CURRENT_TIMESTAMP WHERE username = $2",
			)
			.bind(&challenge.address)
			.bind(&challenge.username)
			.execute(&mut *db)
			.await?;
		} else {
			let current_session = sqlx::query_scalar::<_, String>(
				"SELECT session_id FROM username_v4_owners WHERE username = $1",
			)
			.bind(&challenge.username)
			.fetch_optional(&mut *db)
			.await?;
			if current_session.as_deref() != Some(session) {
				return Err(ErrorResponse::unauthorized(
					"Session does not own this username".into(),
				));
			}
			if challenge.operation == "rename" {
				sqlx::query("DELETE FROM old_names WHERE new_username = $1")
					.bind(&challenge.username)
					.execute(&mut *db)
					.await?;
				sqlx::query("UPDATE names SET username = $1, updated_at = CURRENT_TIMESTAMP WHERE username = $2")
                    .bind(target).bind(&challenge.username).execute(&mut *db).await?;
				sqlx::query("INSERT INTO old_names (old_username,new_username) VALUES ($1,$2)")
					.bind(&challenge.username)
					.bind(target)
					.execute(&mut *db)
					.await?;
			} else {
				sqlx::query("UPDATE names SET address = $1, updated_at = CURRENT_TIMESTAMP WHERE username = $2")
                    .bind(&challenge.address).bind(&challenge.username).execute(&mut *db).await?;
			}
		}
	}
	if let Some(owner) = owner {
		// Unique constraints arbitrate concurrent registration/migration. Never
		// use an upsert: it would let a second session replace an existing owner.
		sqlx::query("INSERT INTO username_v4_owners (owner_nullifier,username,session_id) VALUES ($1,$2,$3)")
            .bind(owner).bind(target).bind(session).execute(&mut *db).await?;
	}
	Ok(())
}

fn invalidation_keys(challenge: &Challenge) -> Vec<String> {
	let mut keys = vec![format!("query_single:{}", challenge.address)];
	if let Some(previous) = &challenge.previous_address {
		keys.push(format!("query_single:{previous}"));
	}
	for name in std::iter::once(&challenge.username).chain(challenge.new_username.iter()) {
		let name = name.to_lowercase();
		keys.extend([
			format!("query_single:{name}"),
			format!("avatar:{name}:original"),
			format!("avatar:{name}:minimized"),
		]);
	}
	keys
}
