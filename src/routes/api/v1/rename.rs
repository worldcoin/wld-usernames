use axum::Extension;
use axum_jsonschema::Json;
use http::StatusCode;
use idkit::session::VerificationLevel;
use tracing::{info_span, Instrument};

use crate::{
	blocklist::BlocklistExt,
	config::{ConfigExt, Db, DEVICE_USERNAME_REGEX, USERNAME_REGEX},
	types::{ErrorResponse, MovedAddress, Name, RenamePayload},
	verify,
};
use redis::{aio::ConnectionManager, AsyncCommands};

#[tracing::instrument(skip_all)]
#[allow(clippy::too_many_lines)] // TODO: refactor
#[allow(dependency_on_unit_never_type_fallback)]
pub async fn rename(
	Extension(config): ConfigExt,
	Extension(db): Extension<Db>,
	Extension(mut redis): Extension<ConnectionManager>,
	Extension(blocklist): BlocklistExt,
	Json(payload): Json<RenamePayload>,
) -> Result<StatusCode, ErrorResponse> {
	let Some(record) = sqlx::query_as!(
		Name,
		"SELECT * FROM names WHERE username = $1",
		&payload.old_username
	)
	.fetch_optional(&db.read_write)
	.instrument(info_span!(
		"rename_check_existing",
		username = payload.old_username
	))
	.await?
	else {
		return Err(ErrorResponse::not_found("Username not found".to_string()));
	};

	if record.nullifier_hash != payload.nullifier_hash {
		return Err(ErrorResponse::unauthorized(
			"You can't update this name".to_string(),
		));
	}

	match verify::dev_portal_verify_proof(
		payload.into_proof(),
		config.wld_app_id.to_string(),
		"username",
		(&payload.old_username, &payload.new_username),
		config.developer_portal_url.clone(),
	)
	.await
	{
		Ok(()) => {},
		Err(verify::Error::Verification(e)) => {
			tracing::error!(
				"Rename Verification Error: {}, payload:{:?}",
				e.detail,
				payload
			);
			return Err(ErrorResponse::validation_error(e.detail));
		},
		Err(e) => {
			tracing::error!(
				"Rename Server Error: {}, payload:{:?}",
				e.to_string(),
				payload
			);
			return Err(ErrorResponse::server_error(
				"Failed to verify World ID proof".to_string(),
			));
		},
	};

	let username_regex = match payload.verification_level.0 {
		VerificationLevel::Orb => USERNAME_REGEX.clone(),
		VerificationLevel::Device => DEVICE_USERNAME_REGEX.clone(),
	};

	if !username_regex.is_match(&payload.new_username) {
		tracing::warn!(
			"Username does not match the required pattern, payload:{:?}",
			payload,
		);
		return Err(ErrorResponse::validation_error(
			"Username does not match the required pattern".to_string(),
		));
	}

	blocklist.ensure_valid(&payload.new_username).map_err(|e| {
		tracing::warn!("Blocklist error, payload:{:?}", payload);
		ErrorResponse::validation_error(e.to_string())
	})?;

	let uniqueness_check = sqlx::query!(
		"SELECT EXISTS(
			SELECT 1 FROM names WHERE LOWER(username) = LOWER($2)
			UNION
			SELECT 1 FROM old_names WHERE LOWER(old_username) = LOWER($2) AND LOWER(new_username) != LOWER($1)
		) AS username",
		&payload.old_username,
		&payload.new_username,
	)
	.fetch_one(&db.read_write)
	.instrument(info_span!(
		"rename_uniqueness_check",
		old_username = payload.old_username,
		new_username = payload.new_username
	))
	.await?;

	if uniqueness_check.username.unwrap_or_default() {
		tracing::warn!("Username is already taken, payload:{:?}", payload);
		return Err(ErrorResponse::validation_error(
			"Username is already taken".to_string(),
		));
	};

	let mut tx = db.read_write.begin().await?;

	// Rename the active name. The `old_names.new_username` foreign key is
	// `ON UPDATE CASCADE`, so any reservation that still points at the old name
	// is automatically repointed to the new one — keeping earlier names in a
	// rename chain reserved instead of orphaning them.
	let Some(moved_address) = sqlx::query_as!(
		MovedAddress,
		"UPDATE names SET username = $1 WHERE username = $2 RETURNING address",
		&payload.new_username,
		&payload.old_username,
	)
	.fetch_optional(&mut *tx)
	.instrument(info_span!(
		"rename_update_name",
		old_username = payload.old_username,
		new_username = payload.new_username
	))
	.await?
	else {
		return Err(ErrorResponse::not_found("Username not found".to_string()));
	};

	reserve_old_name(&mut *tx, &payload.old_username, &payload.new_username).await?;

	tx.commit().await?;

	let query_single_username_cache_key = format!("query_single:{}", payload.old_username);
	let query_single_address_cache_key = format!("query_single:{}", moved_address.address);

	redis
		.del::<_, String>(&query_single_username_cache_key)
		.await?;
	redis
		.del::<_, String>(&query_single_address_cache_key)
		.await?;

	Ok(StatusCode::OK)
}

pub fn docs(op: aide::transform::TransformOperation) -> aide::transform::TransformOperation {
	op.description("Change your World App username to a new one.")
}

/// Records a rename in the `old_names` reservation table within the caller's
/// transaction, so the previous username stays reserved (and resolvable) and
/// cannot be re-registered by anyone else.
///
/// The cleanup `DELETE` is scoped by the username that just became **active**
/// (`new_username`). Scoping it by the name being left behind — as an earlier
/// version did — deleted the reservation of the *original* name in a rename
/// chain (A -> B -> C) and let any other World ID re-register it
/// (HackerOne #3692075).
async fn reserve_old_name(
	tx: &mut sqlx::PgConnection,
	old_username: &str,
	new_username: &str,
) -> Result<(), sqlx::Error> {
	// The new name is active again, so it must not also sit in `old_names`
	// (e.g. when renaming back to a previously held name). The ON UPDATE
	// CASCADE on the rename above can also leave a self-referential row to clear.
	sqlx::query!(
		"DELETE FROM old_names WHERE LOWER(old_username) = LOWER($1)",
		new_username
	)
	.execute(&mut *tx)
	.instrument(info_span!(
		"rename_delete_old_name",
		username = new_username
	))
	.await?;

	// Reserve the name being left behind, repointing it if it somehow already
	// exists so the rename stays idempotent and never hits a primary-key clash.
	sqlx::query!(
		"INSERT INTO old_names (old_username, new_username)
		VALUES ($1, $2)
		ON CONFLICT (old_username) DO UPDATE SET new_username = EXCLUDED.new_username",
		old_username,
		new_username
	)
	.execute(&mut *tx)
	.instrument(info_span!(
		"rename_insert_old_name",
		old_username = old_username,
		new_username = new_username
	))
	.await?;

	Ok(())
}

#[cfg(test)]
mod tests {
	use sqlx::PgPool;

	async fn seed(pool: &PgPool, username: &str) {
		sqlx::query(
			"INSERT INTO names (username, address, nullifier_hash, verification_level)
			VALUES ($1, $2, $3, 'orb')",
		)
		.bind(username)
		.bind("0x0000000000000000000000000000000000000001")
		.bind(format!("nullifier-{username}"))
		.execute(pool)
		.await
		.unwrap();
	}

	/// Mirrors the rename handler's transaction body: rename the active row,
	/// then record the reservation through the production helper.
	async fn do_rename(pool: &PgPool, old: &str, new: &str) {
		let mut tx = pool.begin().await.unwrap();
		sqlx::query("UPDATE names SET username = $1 WHERE username = $2")
			.bind(new)
			.bind(old)
			.execute(&mut *tx)
			.await
			.unwrap();
		super::reserve_old_name(&mut *tx, old, new).await.unwrap();
		tx.commit().await.unwrap();
	}

	/// The same "is this name taken?" predicate `register_username` uses.
	async fn is_reserved(pool: &PgPool, username: &str) -> bool {
		sqlx::query_scalar::<_, bool>(
			"SELECT EXISTS(
				SELECT 1 FROM names WHERE LOWER(username) = LOWER($1)
				UNION
				SELECT 1 FROM old_names WHERE LOWER(old_username) = LOWER($1)
			)",
		)
		.bind(username)
		.fetch_one(pool)
		.await
		.unwrap()
	}

	#[sqlx::test(migrations = "./migrations")]
	async fn original_name_stays_reserved_through_chain(pool: PgPool) {
		seed(&pool, "vitalik").await;
		do_rename(&pool, "vitalik", "vitalik1").await;
		do_rename(&pool, "vitalik1", "vitalik2").await;

		// Regression for HackerOne #3692075: the original name must remain
		// reserved after a multi-rename chain so nobody else can claim it.
		assert!(
			is_reserved(&pool, "vitalik").await,
			"original username was freed after A->B->C"
		);
		assert!(is_reserved(&pool, "vitalik1").await);
		assert!(is_reserved(&pool, "vitalik2").await);
	}

	#[sqlx::test(migrations = "./migrations")]
	async fn rename_back_keeps_name_active_without_self_reference(pool: PgPool) {
		seed(&pool, "alice").await;
		do_rename(&pool, "alice", "alice1").await;
		do_rename(&pool, "alice1", "alice").await;

		assert!(is_reserved(&pool, "alice").await);
		assert!(is_reserved(&pool, "alice1").await);

		let active: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM names WHERE username = 'alice'")
			.fetch_one(&pool)
			.await
			.unwrap();
		assert_eq!(active, 1, "alice should be active again after rename-back");

		let self_refs: i64 =
			sqlx::query_scalar("SELECT COUNT(*) FROM old_names WHERE old_username = new_username")
				.fetch_one(&pool)
				.await
				.unwrap();
		assert_eq!(
			self_refs, 0,
			"rename-back must not leave a self-referential row"
		);
	}
}
