use thiserror::Error;

#[derive(Debug, Error)]
pub enum QueueError {
	#[error("Failed to initialize queue: {0}")]
	InitError(String),
	#[error("Failed to receive messages: {0}")]
	ReceiveMessage(String),
	#[error("Failed to parse message: {0}")]
	ParseMessage(String),
	#[error("Failed to delete message: {0}")]
	DeleteMessage(String),
	#[error("Failed to send message: {0}")]
	SendMessage(String),
	#[error("AWS SDK error: {0}")]
	AwsSdk(String),
	#[error("Missing environment variable: {0}")]
	MissingEnvVar(String),
	#[error("Invalid environment variable: {0}")]
	InvalidEnvVar(String),
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
