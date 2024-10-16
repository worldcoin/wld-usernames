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

	function addr(bytes node) returns (bytes memory);
	function addr(bytes32 node) returns (bytes memory);
	function addr(bytes32 node, uint256 coinType) returns (bytes memory);
	function text(bytes32 node, string key) returns (string);

	struct GatewayResponse {
		address sender;
		uint256 expiresAt;
		bytes32 requestHash;
		bytes32 responseHash;
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
			"85337958" => {
				tracing::info!("addr0 ");
				let addr = addr_0Call::abi_decode(&self.data, true)?;
				Method::Addr(addr.node.to_vec())
			},
			"3b3b57de" => {
				tracing::info!("addr1 ");
				let addr = addr_1Call::abi_decode(&self.data, true)?;
				Method::Addr(addr.node.to_vec())
			},
			"f1cb7e06" => {
				tracing::info!("addr2 ");
				let addr = addr_2Call::abi_decode(&self.data, true)?;
				Method::Addr(addr.node.to_vec())
			},
			"b8f2bbb4" => Method::InterfaceImplementer,
			"59d1d43c" => {
				let addr = textCall::abi_decode(&self.data, true)?;
				Method::Text(addr.node.to_vec(), addr.key)
			},
			_ => {
				tracing::error!("invalid method {:?}", hex::encode(&self.data[..4]).as_str());
				bail!("invalid method {:?}", hex::encode(&self.data[..4]).as_str())
			},
		};

		Ok(method)
	}
}
