use axum::Extension;
use axum_jsonschema::Json;
use http::StatusCode;
use idkit::session::{AppId, VerificationLevel};
use regex::Regex;
use sqlx::PgPool;
use std::{
    env,
    sync::{Arc, LazyLock},
};

use crate::{
    blocklist::Blocklist,
    types::{ErrorResponse, Name, RegisterUsernamePayload},
};

static USERNAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]\w{2,13}[a-z0-9]$").unwrap());
static DEVICE_USERNAME_REGEX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z]\w{2,13}[a-z0-9]\.\d{4}$").unwrap());

#[allow(dependency_on_unit_never_type_fallback)]
pub async fn register_username(
    Extension(db): Extension<PgPool>,
    Extension(blocklist): Extension<Arc<Blocklist>>,
    Json(payload): Json<RegisterUsernamePayload>,
) -> Result<StatusCode, ErrorResponse> {
    match idkit::verify_proof(
        payload.into_proof(),
        unsafe { AppId::new_unchecked(env::var("WORLD_ID_APP_ID").unwrap()) },
        "username",
        (&payload.username, payload.address.0.to_checksum(None)),
    )
    .await
    {
        Ok(()) => {}
        Err(idkit::verify::Error::Verification(e)) => {
            return Err(ErrorResponse::validation_error(e.detail))
        }
        Err(_) => {
            return Err(ErrorResponse::server_error(
                "Failed to verify World ID proof".to_string(),
            ))
        }
    };

    let username_regex = match payload.verification_level.0 {
        VerificationLevel::Orb => USERNAME_REGEX.clone(),
        VerificationLevel::Device => DEVICE_USERNAME_REGEX.clone(),
    };

    if !username_regex.is_match(&payload.username) {
        return Err(ErrorResponse::validation_error(
            "Username does not match the required pattern".to_string(),
        ));
    }

    blocklist
        .ensure_valid(&payload.username)
        .map_err(|e| ErrorResponse::validation_error(e.to_string()))?;

    if sqlx::query_scalar!(
        "SELECT EXISTS (
            SELECT 1 FROM names WHERE username = $1
            UNION
            SELECT 1 FROM old_names WHERE old_username = $1
        )",
        &payload.username
    )
    .fetch_one(&db)
    .await?
        == Some(true)
    {
        return Err(ErrorResponse::validation_error(
            "Username is already taken".to_string(),
        ));
    };

    Name::new(
        payload.username,
        &payload.address,
        payload.nullifier_hash,
        &payload.verification_level,
    )
    .insert(&db, "names")
    .await?;

    Ok(StatusCode::CREATED)
}
