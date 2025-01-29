use async_trait::async_trait;
use aws_sdk_sqs::Client as SqsClient;
use serde::{Deserialize, Deserializer, Serialize};
use tracing::error;
use uuid::Uuid;

use super::error::QueueError;

const SUPPORTED_VERSION: i32 = 1;

#[derive(Debug, Serialize, Deserialize)]
pub struct UserData {
	#[serde(rename = "id")]
	pub id: Uuid,
	#[serde(rename = "publicKeyId")]
	pub public_key_id: String,
	#[serde(rename = "walletAddress")]
	pub wallet_address: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct DataDeletionRequest {
	pub user: UserData,
	#[serde(rename = "correlationId")]
	pub correlation_id: Uuid,
	#[serde(rename = "type")]
	pub message_type: String,
	#[serde(default = "default_version", deserialize_with = "validate_version")]
	pub version: i32,
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
		return Err(serde::de::Error::custom(format!(
			"Unsupported version: {version}. Only version {SUPPORTED_VERSION} is supported",
		)));
	}
	Ok(version)
}

#[derive(Debug)]
pub struct QueueMessage {
	pub request: DataDeletionRequest,
	pub receipt_handle: String,
}

#[async_trait]
pub trait DeletionRequestQueue: Send + Sync {
	async fn poll_messages(&self) -> Result<Vec<QueueMessage>, QueueError>;
	async fn acknowledge(&self, receipt_handle: &str) -> Result<(), QueueError>;
}

pub struct DeletionRequestQueueImpl {
	sqs_client: SqsClient,
	queue_url: String,
	max_messages: i32,
}

impl DeletionRequestQueueImpl {
	async fn init_sqs_client() -> Result<(SqsClient, String, i32), Box<dyn std::error::Error>> {
		let aws_config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
		let sqs_client = SqsClient::new(&aws_config);

		let queue_url = std::env::var("SQS_DELETION_REQUEST_QUEUE_URL")?;
		let max_messages = std::env::var("SQS_DELETION_REQUEST_MAX_MESSAGES")?.parse()?;

		Ok((sqs_client, queue_url, max_messages))
	}

	pub async fn new() -> Result<Self, QueueError> {
		let (sqs_client, queue_url, max_messages) = Self::init_sqs_client()
			.await
			.map_err(|e| QueueError::InitError(e.to_string()))?;

		Ok(Self {
			sqs_client,
			queue_url,
			max_messages,
		})
	}

	fn format_message(message: aws_sdk_sqs::types::Message) -> Result<QueueMessage, QueueError> {
		let body = message
			.body
			.as_ref()
			.ok_or_else(|| QueueError::InvalidMessage("Message body is empty".to_string()))?;

		let receipt_handle = message.receipt_handle.ok_or_else(|| {
			QueueError::InvalidMessage("Message receipt handle is missing".to_string())
		})?;

		let request: DataDeletionRequest = serde_json::from_str(body)
			.map_err(|e| QueueError::InvalidMessage(format!("Failed to parse message: {e}")))?;

		Ok(QueueMessage {
			request,
			receipt_handle,
		})
	}
}

#[async_trait]
impl DeletionRequestQueue for DeletionRequestQueueImpl {
	async fn poll_messages(&self) -> Result<Vec<QueueMessage>, QueueError> {
		let receive_msg_output = self
			.sqs_client
			.receive_message()
			.queue_url(&self.queue_url)
			.wait_time_seconds(20)
			.max_number_of_messages(self.max_messages)
			.send()
			.await
			.map_err(QueueError::SqsError)?;

		let messages = receive_msg_output.messages.unwrap_or_default();

		Ok(messages
			.into_iter()
			.filter_map(|msg| match Self::format_message(msg) {
				Ok(queue_msg) => Some(queue_msg),
				Err(e) => {
					error!("Failed to parse message: {}", e);
					None
				},
			})
			.collect())
	}

	async fn acknowledge(&self, receipt_handle: &str) -> Result<(), QueueError> {
		self.sqs_client
			.delete_message()
			.queue_url(&self.queue_url)
			.receipt_handle(receipt_handle)
			.send()
			.await
			.map_err(|e| QueueError::DeleteMessage(e.to_string()))?;

		Ok(())
	}
}
