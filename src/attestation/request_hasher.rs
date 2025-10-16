use axum::body::Bytes;
use axum::http::HeaderMap;
use futures::stream;
use sha2::{Digest, Sha256};

use super::types::AttestationError;

/// Hash a multipart form data request for attestation verification
/// Computes `SHA256(metadata_json)`
///
/// Expected multipart fields:
/// - metadata: JSON string containing proof, address, `challenge_image_hash`, etc.
/// - `profile_picture`: Image file (ignored for hashing)
pub async fn hash_request(headers: &HeaderMap, body: Bytes) -> Result<String, AttestationError> {
	let content_type = headers
		.get("content-type")
		.and_then(|v| v.to_str().ok())
		.ok_or_else(|| AttestationError::HashError("Missing Content-Type header".to_string()))?;

	// Verify this is multipart form data
	if !content_type.contains("multipart/form-data") {
		return Err(AttestationError::HashError(
			"Only multipart/form-data is supported".to_string(),
		));
	}

	// Extract boundary from content-type
	let boundary = multer::parse_boundary(content_type)
		.map_err(|e| AttestationError::HashError(format!("Invalid boundary: {e}")))?;

	// Convert Bytes to a Stream for multer
	let body_clone = body.clone();
	let stream = stream::once(async move { Ok::<_, std::io::Error>(body_clone) });
	let mut multipart = multer::Multipart::new(stream, boundary);

	let mut metadata_content: Option<Vec<u8>> = None;

	// Parse all form fields looking for metadata
	while let Some(field) = multipart
		.next_field()
		.await
		.map_err(|e| AttestationError::HashError(format!("Multipart parse error: {e}")))?
	{
		let name = field.name().unwrap_or("").to_string();

		if name == "metadata" {
			// Extract metadata JSON as bytes
			let data = field.bytes().await.map_err(|e| {
				AttestationError::HashError(format!("Failed to read metadata: {e}"))
			})?;
			metadata_content = Some(data.to_vec());
			break; // Found metadata, no need to parse other fields
		}
	}

	// metadata is required
	let hash_content = metadata_content
		.ok_or_else(|| AttestationError::HashError("Missing metadata field".to_string()))?;

	// Hash the metadata JSON bytes: SHA256(metadata_json)
	let mut hasher = Sha256::new();
	hasher.update(&hash_content);
	let hash = hasher.finalize();

	Ok(hex::encode(hash))
}
