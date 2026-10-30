//! Canonical identity for legacy World ID nullifiers.
//!
//! Match the Developer Portal's checked nullifier parser. Compare padded hex
//! lexicographically to preserve all 256 bits without reducing modulo the field.

const FIELD_MODULUS_HEX: &str =
	"30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000001";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidNullifier;

impl std::fmt::Display for InvalidNullifier {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str("Invalid World ID nullifier: expected 1–64 hex digits within the World ID field")
	}
}

impl std::error::Error for InvalidNullifier {}

/// Parse optional-prefix hex into exactly 32 lowercase bytes of hex.
///
/// # Errors
/// Rejects non-hex input, whitespace, oversized encodings and values outside
/// the BN254 scalar field. Bare digits are hexadecimal, never decimal.
pub fn canonical_nullifier(input: &str) -> Result<String, InvalidNullifier> {
	let digits = input
		.strip_prefix("0x")
		.or_else(|| input.strip_prefix("0X"))
		.unwrap_or(input);
	if digits.is_empty() || digits.len() > 64 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
		return Err(InvalidNullifier);
	}
	let padded = format!("{:0>64}", digits.to_ascii_lowercase());
	if padded.as_str() >= FIELD_MODULUS_HEX {
		return Err(InvalidNullifier);
	}
	Ok(format!("0x{padded}"))
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn equivalent_encodings_have_one_identity() {
		let expected = format!("0x{:0>64}", "1a");
		for input in ["0x01a", "0x1a", "0x1A", "0X1a", "1a", "00001A"] {
			assert_eq!(canonical_nullifier(input).unwrap(), expected);
		}
		assert_eq!(canonical_nullifier("10").unwrap(), format!("0x{:0>64}", "10"));
	}

	#[test]
	fn preserves_exact_field_boundaries_and_large_integers() {
		assert_eq!(canonical_nullifier("0").unwrap(), format!("0x{}", "0".repeat(64)));
		let largest = "30644e72e131a029b85045b68181585d2833e84879b9709143e1f593f0000000";
		assert_eq!(canonical_nullifier(largest).unwrap(), format!("0x{largest}"));
		assert_eq!(canonical_nullifier(&largest.to_uppercase()).unwrap(), format!("0x{largest}"));
		assert_ne!(canonical_nullifier("20000000000001"), canonical_nullifier("20000000000000"));
	}

	#[test]
	fn rejects_invalid_encodings_and_field_overflow() {
		for input in ["", "0x", "0X", " 0x1", "0x1 ", "1\n", "\t1", "-1", "+1", "0xg", "0x0x1", "１", FIELD_MODULUS_HEX] {
			assert_eq!(canonical_nullifier(input), Err(InvalidNullifier), "{input:?}");
		}
		for input in ["f".repeat(64), "0".repeat(65), "1".repeat(65)] {
			assert_eq!(canonical_nullifier(&input), Err(InvalidNullifier));
		}
	}
}
