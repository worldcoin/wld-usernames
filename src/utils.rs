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

pub fn decode_ens_name(name: &str) -> String {
	let mut labels: Vec<&str> = Vec::new();
	let mut idx = 0;
	loop {
		let len = name.as_bytes()[idx] as usize;
		if len == 0 {
			break;
		}
		labels.push(&name[(idx + 1)..=(idx + len)]);
		idx += len + 1;
	}

	labels.join(".")
}
