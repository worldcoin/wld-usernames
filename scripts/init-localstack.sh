#!/bin/bash

# Wait for LocalStack to be ready
echo "Waiting for LocalStack to be ready..."
while ! nc -z localhost 4566; do
  sleep 1
done
echo "LocalStack is ready!"

# Create SQS queue
echo "Creating SQS queue..."
aws --endpoint-url=http://localhost:4566 \
    --region us-east-1 \
    sqs create-queue \
    --queue-name wld-username-deletion-requests-local \
    --attributes '{
      "VisibilityTimeout": "30",
      "MessageRetentionPeriod": "86400",
      "ReceiveMessageWaitTimeSeconds": "20"
    }'

echo "LocalStack initialization completed!" 
