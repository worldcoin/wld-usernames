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
			if metadata_content.is_some() {
				return Err(AttestationError::HashError(
					"Duplicate metadata field".to_string(),
				));
			}

			// Extract metadata JSON as bytes
			let data = field.bytes().await.map_err(|e| {
				AttestationError::HashError(format!("Failed to read metadata: {e}"))
			})?;
			metadata_content = Some(data.to_vec());
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

#[cfg(test)]
mod tests {
	use super::*;
	use axum::http::header::CONTENT_TYPE;

	fn multipart_headers(boundary: &str) -> HeaderMap {
		let mut headers = HeaderMap::new();
		headers.insert(
			CONTENT_TYPE,
			format!("multipart/form-data; boundary={boundary}")
				.parse()
				.expect("valid header"),
		);
		headers
	}

	#[tokio::test]
	async fn hash_request_errors_on_duplicate_metadata_parts() {
		let boundary = "boundary123";
		let headers = multipart_headers(boundary);
		let body = [
			format!(
				"--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{{\"value\":1}}\r\n"
			),
			format!(
				"--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{{\"value\":2}}\r\n"
			),
			format!("--{boundary}--\r\n"),
		]
		.join("");
		let body = Bytes::from(body);

		let err = hash_request(&headers, body).await.expect_err("should error");

		match err {
			AttestationError::HashError(message) => {
				assert!(message.contains("Duplicate metadata"), "{message}");
			},
			_ => panic!("unexpected error variant"),
		}
	}

	#[tokio::test]
	async fn hash_request_hashes_single_metadata_part() {
		let boundary = "boundary456";
		let headers = multipart_headers(boundary);
		let metadata = r#"{"value":1}"#;
		let body = [
			format!(
				"--{boundary}\r\nContent-Disposition: form-data; name=\"metadata\"\r\n\r\n{metadata}\r\n"
			),
			format!(
				"--{boundary}\r\nContent-Disposition: form-data; name=\"profile_picture\"\r\n\r\n<bytes>\r\n"
			),
			format!("--{boundary}--\r\n"),
		]
		.join("");
		let body = Bytes::from(body.clone());

		let hash = hash_request(&headers, body).await.expect("hash succeeds");

		let mut hasher = Sha256::new();
		hasher.update(metadata.as_bytes());
		let expected = hex::encode(hasher.finalize());

		assert_eq!(hash, expected);
	}
}
