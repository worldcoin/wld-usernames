//! Canonical identity for legacy World ID nullifiers.
//!
//! Keep identity as checked, big-endian bytes; hexadecimal is only a wire format.

const FIELD_MODULUS: [u8; 32] = [
	0x30, 0x64, 0x4e, 0x72, 0xe1, 0x31, 0xa0, 0x29, 0xb8, 0x50, 0x45, 0xb6, 0x81, 0x81, 0x58, 0x5d,
	0x28, 0x33, 0xe8, 0x48, 0x79, 0xb9, 0x70, 0x91, 0x43, 0xe1, 0xf5, 0x93, 0xf0, 0x00, 0x00, 0x01,
];

/// A checked BN254 scalar-field identity, independent of its wire encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Nullifier([u8; 32]);

impl Nullifier {
	pub fn as_bytes(&self) -> &[u8] {
		&self.0
	}
}

impl std::fmt::Display for Nullifier {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "0x{}", hex::encode(self.0))
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidNullifier;

impl std::fmt::Display for InvalidNullifier {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(
			"Invalid World ID nullifier: expected 1–64 hex digits within the World ID field",
		)
	}
}

impl std::error::Error for InvalidNullifier {}

/// Parse optional-prefix hex into exactly 32 checked big-endian bytes.
///
/// # Errors
/// Rejects non-hex input, whitespace, oversized encodings and values outside
/// the BN254 scalar field. Bare digits are hexadecimal, never decimal.
pub fn canonical_nullifier(input: &str) -> Result<Nullifier, InvalidNullifier> {
	let digits = input
		.strip_prefix("0x")
		.or_else(|| input.strip_prefix("0X"))
		.unwrap_or(input);
	if digits.is_empty() || digits.len() > 64 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
		return Err(InvalidNullifier);
	}
	let mut padded = [b'0'; 64];
	padded[64 - digits.len()..].copy_from_slice(digits.as_bytes());
	let mut bytes = [0; 32];
	hex::decode_to_slice(padded, &mut bytes).map_err(|_| InvalidNullifier)?;
	if bytes >= FIELD_MODULUS {
		return Err(InvalidNullifier);
	}
	Ok(Nullifier(bytes))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn equivalent_encodings_have_one_identity() {
		let expected = format!("0x{:0>64}", "1a");
		for input in ["0x01a", "0x1a", "0x1A", "0X1a", "1a", "00001A"] {
			assert_eq!(canonical_nullifier(input).unwrap().to_string(), expected);
		}
		assert_eq!(
			canonical_nullifier("10").unwrap().to_string(),
			format!("0x{:0>64}", "10")
		);
	}

	#[test]
	fn identity_is_fixed_width_big_endian_bytes() {
		let value = canonical_nullifier("0X01a").unwrap();
		let mut expected = [0; 32];
		expected[31] = 0x1a;
		assert_eq!(value.as_bytes(), expected);
		assert_eq!(std::mem::size_of::<Nullifier>(), 32);
		assert_eq!(value, canonical_nullifier("1A").unwrap());
		assert_ne!(value, canonical_nullifier("1b").unwrap());
		assert_eq!(canonical_nullifier("100").unwrap().as_bytes()[30..], [1, 0]);
		assert_eq!(canonical_nullifier("0").unwrap().as_bytes(), [0; 32]);
	}

	#[test]
	fn preserves_exact_field_boundaries_and_large_integers() {
		assert_eq!(
			canonical_nullifier("0").unwrap().to_string(),
			format!("0x{}", "0".repeat(64))
		);
		let largest = "30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000000";
		assert_eq!(
			canonical_nullifier(largest).unwrap().to_string(),
			format!("0x{largest}")
		);
		assert_eq!(
			canonical_nullifier(&largest.to_uppercase())
				.unwrap()
				.to_string(),
			format!("0x{largest}")
		);
		assert_ne!(
			canonical_nullifier("20000000000001"),
			canonical_nullifier("20000000000000")
		);
	}

	#[test]
	fn rejects_invalid_encodings_and_field_overflow() {
		for input in [
			"",
			"0x",
			"0X",
			" 0x1",
			"0x1 ",
			"1\n",
			"\t1",
			"-1",
			"+1",
			"0xg",
			"0x0x1",
			"１",
			"30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000001",
		] {
			assert_eq!(
				canonical_nullifier(input),
				Err(InvalidNullifier),
				"{input:?}"
			);
		}
		for input in ["f".repeat(64), "0".repeat(65), "1".repeat(65)] {
			assert_eq!(canonical_nullifier(&input), Err(InvalidNullifier));
		}
	}
}
