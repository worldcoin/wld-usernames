use axum::{
    extract::Path,
    response::{IntoResponse, Redirect, Response},
    Extension,
};
use axum_jsonschema::Json;
use sqlx::PgPool;

use crate::types::{ErrorResponse, MovedRecord, Name};

pub async fn query_single(
    Extension(db): Extension<PgPool>,
    Path(name_or_address): Path<String>,
) -> Result<Response, ErrorResponse> {
    if let Some(name) = sqlx::query_as!(
        Name,
        "SELECT * FROM names WHERE username = $1 OR address = $1",
        name_or_address
    )
    .fetch_optional(&db)
    .await?
    {
        return Ok(Json(name).into_response());
    };

    if let Some(moved) = sqlx::query_as!(
        MovedRecord,
        "SELECT * FROM old_names WHERE old_username = $1",
        name_or_address
    )
    .fetch_optional(&db)
    .await?
    {
        return Ok(Redirect::permanent(&format!("/api/v1/{}", moved.new_username)).into_response());
    }

    Err(ErrorResponse::not_found("Record not found.".to_string()))
}
