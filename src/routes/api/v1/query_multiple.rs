use axum::Extension;
use axum_jsonschema::Json;
use sqlx::PgPool;

use crate::types::{ErrorResponse, Name, QueryAddressesPayload};

pub async fn query_multiple(
    Extension(db): Extension<PgPool>,
    Json(payload): Json<QueryAddressesPayload>,
) -> Result<Json<Vec<Name>>, ErrorResponse> {
    let addresses = payload
        .addresses
        .iter()
        .map(|a| a.0.to_checksum(None))
        .collect::<Vec<_>>();

    let names = sqlx::query_as!(
        Name,
        "SELECT * FROM names WHERE address = ANY($1)",
        &addresses
    )
    .fetch_all(&db)
    .await?;

    Ok(Json(names))
}
