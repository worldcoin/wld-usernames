use async_trait::async_trait;
use aws_config::{BehaviorVersion, Region};
use aws_sdk_sqs::{config::Credentials, Client as SqsClient, Config};
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
		let sqs_client = if std::env::var("ENV").unwrap_or_default() == "local" {
			let aws_config = Config::builder()
				.region(Region::new(
					std::env::var("AWS_REGION").expect("AWS_REGION is not set"),
				))
				.credentials_provider(Credentials::new(
					std::env::var("AWS_ACCESS_KEY_ID").expect("AWS_ACCESS_KEY_ID is not set"),
					std::env::var("AWS_SECRET_ACCESS_KEY")
						.expect("AWS_SECRET_ACCESS_KEY is not set"),
					None,
					None,
					"static",
				))
				.endpoint_url(std::env::var("AWS_ENDPOINT").expect("AWS_ENDPOINT is not set"))
				.behavior_version(BehaviorVersion::latest())
				.build();
			SqsClient::from_conf(aws_config)
		} else {
			let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
			SqsClient::new(&aws_config)
		};
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
