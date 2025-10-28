use aws_sdk_s3::types::{Tag, Tagging};
use tracing::{info, warn};

use crate::config::Config;

const DELETION_TAG_KEY: &str = "pending-deletion";
const DELETION_TAG_VALUE: &str = "true";

pub async fn mark_object_for_deletion(config: &Config, cdn_base_url: &str, url: &str) {
	let Some(object_key) = object_key_from_cdn_url(cdn_base_url, url) else {
		return;
	};

	let Ok(bucket) = std::env::var("UPLOADS_BUCKET_NAME") else {
		warn!("UPLOADS_BUCKET_NAME environment variable not set; skipping S3 tagging");
		return;
	};

	let tag = match Tag::builder()
		.key(DELETION_TAG_KEY)
		.value(DELETION_TAG_VALUE)
		.build()
	{
		Ok(tag) => tag,
		Err(err) => {
			warn!(error = %err, "Failed to construct deletion tag payload");
			return;
		},
	};

	let tagging = match Tagging::builder().set_tag_set(Some(vec![tag])).build() {
		Ok(tagging) => tagging,
		Err(err) => {
			warn!(error = %err, "Failed to construct tagging payload");
			return;
		},
	};

	if let Err(err) = config
		.s3_client()
		.put_object_tagging()
		.bucket(&bucket)
		.key(&object_key)
		.tagging(tagging)
		.send()
		.await
	{
		warn!(
			error = %err,
			bucket = %bucket,
			key = %object_key,
			"Failed to tag profile picture object for deletion"
		);
	} else {
		info!(
			bucket = %bucket,
			key = %object_key,
			"Tagged profile picture object for deferred deletion"
		);
	}
}

fn object_key_from_cdn_url(cdn_base_url: &str, full_url: &str) -> Option<String> {
	let base_url = url::Url::parse(cdn_base_url).ok()?;
	let url = url::Url::parse(full_url).ok()?;

	if base_url.scheme() != url.scheme()
		|| base_url.host_str() != url.host_str()
		|| base_url.port_or_known_default() != url.port_or_known_default()
	{
		return None;
	}

	let base_path = base_url.path().trim_end_matches('/');
	let full_path = url.path();

	let relative_path = if base_path.is_empty() || base_path == "/" {
		full_path.trim_start_matches('/')
	} else {
		full_path.strip_prefix(base_path)?.trim_start_matches('/')
	};

	if relative_path.is_empty() {
		None
	} else {
		Some(relative_path.to_string())
	}
}

#[cfg(test)]
mod tests {
	use super::object_key_from_cdn_url;

	#[test]
	fn derives_relative_path_when_base_has_no_path() {
		let base = "https://cdn.example.com";
		let full = "https://cdn.example.com/foo/bar.png";

		assert_eq!(
			object_key_from_cdn_url(base, full),
			Some("foo/bar.png".to_string())
		);
	}

	#[test]
	fn handles_marble() {
		let base = "https://static.usernames.app-backend.toolsforhumanity.com";
		let full = "https://static.usernames.app-backend.toolsforhumanity.com/0x377da9cab87c04a1d6f19d8b4be9aef8df26fcdd.png";

		assert_eq!(
			object_key_from_cdn_url(base, full),
			Some("0x377da9cab87c04a1d6f19d8b4be9aef8df26fcdd.png".to_string())
		);
	}

	#[test]
	fn handles_profile_picture() {
		let base = "https://assets.usernames.worldcoin.org";
		let full = "https://assets.usernames.worldcoin.org/0x6c5fac447d4d49ec91c24563209184c9a0b1f9da/profile";

		assert_eq!(
			object_key_from_cdn_url(base, full),
			Some("0x6c5fac447d4d49ec91c24563209184c9a0b1f9da/profile".to_string())
		);
	}

	#[test]
	fn rejects_different_hosts() {
		let base = "https://cdn.example.com";
		let full = "https://evil.example.com/foo.png";

		assert_eq!(object_key_from_cdn_url(base, full), None);
	}

	#[test]
	fn rejects_non_matching_paths() {
		let base = "https://cdn.example.com/base";
		let full = "https://cdn.example.com/other/foo.png";

		assert_eq!(object_key_from_cdn_url(base, full), None);
	}
}
