#![allow(clippy::pub_underscore_fields)]

use alloy::sol_types::{sol, SolCall};
use anyhow::bail;
use std::string::FromUtf8Error;

use crate::utils::decode_ens_name;

sol! {
	#![sol(alloy_sol_types = ::alloy::sol_types)]

	function resolve(
		bytes calldata name,
		bytes calldata data
	);

	function addr(bytes32 node) returns (address);
	function text(bytes32 node, string key) returns (string);

	struct GatewayResponse {
		address sender;
		uint256 expires_at;
		bytes32 request_hash;
		bytes32 response_hash;
	}
}

pub enum Method {
	Abi,
	Name,
	PubKey,
	ContentHash,
	Addr(Vec<u8>),
	AddrMultichain,
	InterfaceImplementer,
	Text(Vec<u8>, String),
}

impl resolveCall {
	pub fn parse_name(&self) -> Result<String, FromUtf8Error> {
		Ok(decode_ens_name(&String::from_utf8(self.name.to_vec())?))
	}

	pub fn parse_method(&self) -> anyhow::Result<Method> {
		let method = match hex::encode(&self.data[..4]).as_str() {
			"2203ab56" => Method::Abi,
			"691f3431" => Method::Name,
			"c8690233" => Method::PubKey,
			"bc1c58d1" => Method::ContentHash,
			"f1cb7e06" => Method::AddrMultichain,
			"b8f2bbb4" => Method::InterfaceImplementer,
			"3b3b57de" => {
				let addr = addrCall::abi_decode(&self.data, true)?;
				Method::Addr(addr.node.to_vec())
			},
			"59d1d43c" => {
				let addr = textCall::abi_decode(&self.data, true)?;
				Method::Text(addr.node.to_vec(), addr.key)
			},
			_ => bail!("invalid method"),
		};

		Ok(method)
	}
}
