use async_trait::async_trait;
use aws_sdk_sqs::Client as SqsClient;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::error::QueueError;

const SUPPORTED_VERSION: i32 = 1;
const SERVICE: &str = "wld-usernames";

#[derive(Debug, Serialize, Deserialize)]
pub struct DataDeletionCompletion {
	#[serde(rename = "correlationId")]
	pub correlation_id: Uuid,
	pub service: String,
	#[serde(rename = "completedAt")]
	pub completed_at: DateTime<Utc>,
	#[serde(default = "default_version", deserialize_with = "validate_version")]
	pub version: i32,
}

impl DataDeletionCompletion {
	pub fn new(correlation_id: Uuid) -> Self {
		Self {
			correlation_id,
			service: SERVICE.to_string(),
			completed_at: Utc::now(),
			version: SUPPORTED_VERSION,
		}
	}
}

const fn default_version() -> i32 {
	SUPPORTED_VERSION
}

fn validate_version<'de, D>(deserializer: D) -> Result<i32, D::Error>
where
	D: Deserializer<'de>,
{
	let version = i32::deserialize(deserializer)?;
	if version != SUPPORTED_VERSION {
		return Err(serde::de::Error::custom(
			"Unsupported version: {version}. Only version {SUPPORTED_VERSION} is supported",
		));
	}
	Ok(version)
}

#[async_trait]
pub trait DeletionCompletionQueue: Send + Sync {
	async fn send_message(&self, completion: DataDeletionCompletion) -> Result<(), QueueError>;
}

#[allow(clippy::module_name_repetitions)]
pub struct DeletionCompletionQueueImpl {
	sqs_client: SqsClient,
	queue_url: String,
}

impl DeletionCompletionQueueImpl {
	async fn init_sqs_client() -> Result<(SqsClient, String), Box<dyn std::error::Error>> {
		let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
		let sqs_client = SqsClient::new(&aws_config);

		let queue_url = std::env::var("SQS_DELETION_COMPLETION_QUEUE_URL")?;

		Ok((sqs_client, queue_url))
	}

	pub async fn new() -> Result<Self, QueueError> {
		let (sqs_client, queue_url) = Self::init_sqs_client()
			.await
			.map_err(|e| QueueError::InitError(e.to_string()))?;

		Ok(Self {
			sqs_client,
			queue_url,
		})
	}
}

#[async_trait]
impl DeletionCompletionQueue for DeletionCompletionQueueImpl {
	async fn send_message(&self, completion: DataDeletionCompletion) -> Result<(), QueueError> {
		let message_body = serde_json::to_string(&completion)
			.map_err(|e| QueueError::InvalidMessage(format!("Failed to serialize message: {e}")))?;

		self.sqs_client
			.send_message()
			.queue_url(&self.queue_url)
			.message_body(message_body)
			.send()
			.await
			.map_err(|e| QueueError::SendMessage(e.to_string()))?;

		Ok(())
	}
}
