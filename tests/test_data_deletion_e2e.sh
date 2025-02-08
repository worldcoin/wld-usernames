#!/bin/bash

# Exit on error
set -e

# Load environment variables from .env file
if [ -f .env ]; then
    export $(cat .env | grep -v '^#' | xargs)
else
    echo "Error: .env file not found"
    exit 1
fi

# AWS SQS configuration (make sure these match your local environment)
export AWS_ACCESS_KEY_ID="${AWS_ACCESS_KEY_ID:-test}"
export AWS_SECRET_ACCESS_KEY="${AWS_SECRET_ACCESS_KEY:-test}"
export AWS_DEFAULT_REGION="${AWS_DEFAULT_REGION:-us-east-1}"

echo "AWS_ACCESS_KEY_ID: $AWS_ACCESS_KEY_ID"
echo "AWS_SECRET_ACCESS_KEY: $AWS_SECRET_ACCESS_KEY"
echo "AWS_DEFAULT_REGION: $AWS_DEFAULT_REGION"

AWS_ENDPOINT="${LOCALSTACK_ENDPOINT:-http://localhost:4566}"

# Queue URLs and Topic ARN
REQUEST_QUEUE_URL="$AWS_ENDPOINT/000000000000/wld-username-deletion-requests-local"
COMPLETION_QUEUE_URL="$AWS_ENDPOINT/000000000000/wld-username-deletion-completion-local"
TOPIC_ARN="arn:aws:sns:${AWS_DEFAULT_REGION}:000000000000:wld-username-deletion-requests-local"

# Cleanup queues before starting
echo "Cleaning up queues..."
cleanup_queue() {
    local queue_url=$1
    local queue_name=$2
    
    echo "Purging $queue_name queue..."
    aws --endpoint-url=$AWS_ENDPOINT sqs purge-queue --queue-url "$queue_url" || {
        echo "Warning: Failed to purge $queue_name queue. Attempting to receive and delete all messages..."
        # Receive and delete all messages as fallback
        while true; do
            local messages=$(aws --endpoint-url=$AWS_ENDPOINT sqs receive-message --queue-url "$queue_url" --max-number-of-messages 10 --wait-time-seconds 1)
            if [[ ! $messages == *"ReceiptHandle"* ]]; then
                break
            fi
            echo "Deleting messages from $queue_name queue..."
            echo "$messages" | grep -o '"ReceiptHandle": "[^"]*' | cut -d'"' -f4 | while read -r receipt; do
                aws --endpoint-url=$AWS_ENDPOINT sqs delete-message --queue-url "$queue_url" --receipt-handle "$receipt"
            done
        done
    }
}

cleanup_queue "$REQUEST_QUEUE_URL" "request"
cleanup_queue "$COMPLETION_QUEUE_URL" "completion"

# Test data
TEST_USERNAME="test_user_$(date +%s)"
TEST_ADDRESS="0x23aA57F8a10c570B1FF437a08A017069B9e18aFB"
TEST_NULLIFIER_HASH="0x9876543210fedcba9876543210fedcba98765432"
CORRELATION_ID=$(uuidgen | tr '[:upper:]' '[:lower:]')
USER_ID=$(uuidgen)
PUBLIC_KEY_ID="7557092d349a24cf5b43ac87a76b1c985f5c5b2fe0ecbf1ed22fad77c79f8e0c"

echo "Starting E2E test for username deletion..."
echo "Test username: $TEST_USERNAME"
echo "Correlation ID: $CORRELATION_ID"

# Create temporary SQL files
cat > insert_query.sql << EOF
INSERT INTO names (username, address, nullifier_hash, verification_level) 
VALUES ('$TEST_USERNAME', '$TEST_ADDRESS', '$TEST_NULLIFIER_HASH', 'VERIFIED');
EOF

cat > check_query.sql << EOF
SELECT COUNT(*) FROM names WHERE username = '$TEST_USERNAME';
EOF

# Create username in database
echo "Creating test username in database..."
PGPASSWORD=$POSTGRES_PASSWORD psql "${DATABASE_URL}" -f insert_query.sql

# Send deletion request message to SNS
echo "Sending deletion request to SNS..."
REQUEST_PAYLOAD='{
  "user": {
    "id": "'$USER_ID'",
    "publicKeyId": "'$PUBLIC_KEY_ID'",
    "walletAddress": "'$TEST_ADDRESS'"
  },
  "correlationId": "'$CORRELATION_ID'",
  "deletionType": "data_deletion",
  "version": 1
}'

# Send message directly without additional escaping
aws --endpoint-url=$AWS_ENDPOINT sns publish \
    --topic-arn "$TOPIC_ARN" \
    --message "$REQUEST_PAYLOAD"

echo "Starting polling loop..."
MAX_ATTEMPTS=30
ATTEMPT=0
SUCCESS=false

while [ $ATTEMPT -lt $MAX_ATTEMPTS ]; do
    echo -e "\nPolling attempt $((ATTEMPT+1))/$MAX_ATTEMPTS..."
    
    # Check if username was deleted
    USERNAME_EXISTS=$(PGPASSWORD=$POSTGRES_PASSWORD psql "${DATABASE_URL}"  -t -A -f check_query.sql)
    
    # Check completion message
    COMPLETION_MESSAGE=$(aws --endpoint-url=$AWS_ENDPOINT sqs receive-message \
        --queue-url "$COMPLETION_QUEUE_URL" \
        --wait-time-seconds 1\
        --visibility-timeout 0)
    
    # Check if request message was deleted (by receiving it - if we can't receive it, it's likely deleted)
    REQUEST_MESSAGE=$(aws --endpoint-url=$AWS_ENDPOINT sqs receive-message \
        --queue-url "$REQUEST_QUEUE_URL" \
        --wait-time-seconds 1 \
        --visibility-timeout 0)

    # Log current status
    echo "Current status:"
    if [ "$USERNAME_EXISTS" = "0" ]; then
        echo "✓ Username deleted from database"
    else
        echo "✗ Username still exists in database"
    fi

    if [[ $COMPLETION_MESSAGE == *"$CORRELATION_ID"* ]]; then
        echo "✓ Completion message contains correct correlation ID"
    else
        echo "✗ Completion message with correct correlation ID not found"
    fi

    if [[ $COMPLETION_MESSAGE == *"wld-usernames"* ]]; then
        echo "✓ Completion message contains correct service name"
    else
        echo "✗ Completion message missing correct service name"
    fi

    if [[ $COMPLETION_MESSAGE == *"completedAt"* ]]; then
        echo "✓ Completion message contains completedAt timestamp"
    else
        echo "✗ Completion message missing completedAt timestamp"
    fi

    if [[ $COMPLETION_MESSAGE == *"version"* ]]; then
        echo "✓ Completion message contains version"
    else
        echo "✗ Completion message missing version"
    fi

    if [[ ! $REQUEST_MESSAGE == *"$CORRELATION_ID"* ]]; then
        echo "✓ Original request message deleted"
    else
        echo "✗ Original request message still exists"
    fi
    
    # Check if all conditions are met
    if [ "$USERNAME_EXISTS" = "0" ] && \
       [[ $COMPLETION_MESSAGE == *"$CORRELATION_ID"* ]] && \
       [[ $COMPLETION_MESSAGE == *"wld-usernames"* ]] && \
       [[ $COMPLETION_MESSAGE == *"completedAt"* ]] && \
       [[ $COMPLETION_MESSAGE == *"version"* ]] && \
       [[ ! $REQUEST_MESSAGE == *"$CORRELATION_ID"* ]]; then
        echo -e "\nSuccess! All conditions met!"
        SUCCESS=true
        break
    fi
    
    ATTEMPT=$((ATTEMPT+1))
    if [ $ATTEMPT -lt $MAX_ATTEMPTS ]; then
        echo "Waiting 10 seconds before next attempt..."
        sleep 10
    fi
done

if [ "$SUCCESS" = false ]; then
    echo -e "\nTest failed after $MAX_ATTEMPTS attempts!"
    echo "Final completion message received: $COMPLETION_MESSAGE"
    exit 1
fi

# Cleanup temporary files
rm -f insert_query.sql check_query.sql

echo -e "\nE2E test completed successfully!" 
