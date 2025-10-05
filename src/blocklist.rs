use axum::Extension;
use std::{collections::HashSet, sync::Arc};

#[allow(clippy::module_name_repetitions)]
pub type BlocklistExt = Extension<Arc<Blocklist>>;

/// A blocklist of usernames and substrings.
#[derive(Debug)]
pub struct Blocklist {
	/// A list of reserved usernames
	names: HashSet<Box<str>>,
	/// A list of substrings that are not allowed in usernames
	substrings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("The requested username is reserved.")]
	Reserved,
	#[error("Usernames cannot contain the word \"{0}\".")]
	Contains(String),
}

impl Blocklist {
	/// Create a new blocklist from the given strings.
	/// - `blocked_names` is a comma-separated list of blocked usernames
	/// - `blocked_substrings` is a comma-separated list of blocked substrings
	pub fn new(blocked_names: &str, blocked_substrings: &str) -> Self {
		let names = blocked_names.split(',').map(|s| s.trim().into()).collect();
		let substrings = blocked_substrings
			.split(',')
			.map(|s| s.trim().into())
			.collect();

		Self { names, substrings }
	}

	/// Check if a username is blocked.
	pub fn ensure_valid(&self, username: &str) -> Result<(), Error> {
		if self.names.contains(username.to_lowercase().as_str()) {
			return Err(Error::Reserved);
		};

		if let Some(substring) = self
			.substrings
			.iter()
			.find(|s| username.contains(s.as_str()))
		{
			return Err(Error::Contains(substring.clone()));
		};

		Ok(())
	}
}
