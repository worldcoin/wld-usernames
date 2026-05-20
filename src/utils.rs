use alloy::primitives::keccak256;

pub fn namehash(name: &str) -> [u8; 32] {
	if name.is_empty() {
		return [0; 32];
	}

	// Remove the variation selector U+FE0F
	let name = name.replace('\u{fe0f}', "");

	// Generate the node starting from the right
	name.rsplit('.').fold([0u8; 32], |node, label| {
		*keccak256([node, *keccak256(label.as_bytes())].concat())
	})
}

pub fn decode_ens_name(name: &str) -> anyhow::Result<String> {
	use anyhow::Context;

	let bytes = name.as_bytes();
	let mut labels: Vec<&str> = Vec::new();
	let mut idx = 0;
	loop {
		let len = *bytes
			.get(idx)
			.context("truncated DNS-encoded ENS name")? as usize;
		if len == 0 {
			break;
		}
		let start = idx + 1;
		let end = start + len;
		let label = name
			.get(start..end)
			.context("invalid DNS-encoded ENS name label")?;
		labels.push(label);
		idx = end;
	}

	Ok(labels.join("."))
}

pub const ONE_MINUTE_IN_SECONDS: u64 = 60;

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn decode_ens_name_round_trip() {
		assert_eq!(
			decode_ens_name("\x04test\x03eth\x00").unwrap(),
			"test.eth"
		);
	}

	#[test]
	fn decode_ens_name_just_terminator() {
		assert_eq!(decode_ens_name("\x00").unwrap(), "");
	}

	#[test]
	fn decode_ens_name_empty_input() {
		assert!(decode_ens_name("").is_err());
	}

	#[test]
	fn decode_ens_name_missing_terminator() {
		assert!(decode_ens_name("\x04test").is_err());
	}

	#[test]
	fn decode_ens_name_length_overruns_input() {
		assert!(decode_ens_name("\x0aab").is_err());
	}

	#[test]
	fn decode_ens_name_label_splits_utf8_char() {
		assert!(decode_ens_name("\x01\u{00c8}\x00").is_err());
	}
}
