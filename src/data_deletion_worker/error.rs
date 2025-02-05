use thiserror::Error;

#[allow(clippy::module_name_repetitions)]
#[derive(Debug, Error)]
pub enum QueueError {
	#[error("Failed to initialize queue: {0}")]
	InitError(String),
	#[error("Failed to delete message: {0}")]
	DeleteMessage(String),
	#[error("Failed to send message: {0}")]
	SendMessage(String),
	#[error("Invalid message: {0}")]
	InvalidMessage(String),
	#[error("SQS error: {0}")]
	SqsError(
		#[from]
		aws_sdk_sqs::error::SdkError<aws_sdk_sqs::operation::receive_message::ReceiveMessageError>,
	),
	#[error("Database error: {0}")]
	DatabaseError(#[from] sqlx::Error),
}
