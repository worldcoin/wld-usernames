#!/bin/sh

# Create SQS queues
awslocal sqs create-queue --queue-name wld-username-deletion-requests-local
awslocal sqs create-queue --queue-name wld-username-deletion-completion-local

# Get queue ARN
QUEUE_URL=$(awslocal sqs get-queue-url --queue-name wld-username-deletion-requests-local --query 'QueueUrl' --output text)
QUEUE_ARN=$(awslocal sqs get-queue-attributes --queue-url $QUEUE_URL --attribute-names QueueArn --query 'Attributes.QueueArn' --output text)

# Create SNS topic
TOPIC_ARN=$(awslocal sns create-topic --name wld-username-deletion-requests-local --query 'TopicArn' --output text)

# Subscribe SQS to SNS
awslocal sns subscribe \
    --topic-arn $TOPIC_ARN \
    --protocol sqs \
    --notification-endpoint $QUEUE_ARN \
    --attributes RawMessageDelivery=false

# Set SQS queue policy to allow SNS (as a single line with minimal escaping)
awslocal sqs set-queue-attributes \
    --queue-url $QUEUE_URL \
    --attributes '{"Policy":"{\"Version\":\"2012-10-17\",\"Statement\":[{\"Effect\":\"Allow\",\"Principal\":{\"Service\":\"sns.amazonaws.com\"},\"Action\":\"sqs:SendMessage\",\"Resource\":\"'$QUEUE_ARN'\",\"Condition\":{\"ArnEquals\":{\"aws:SourceArn\":\"'$TOPIC_ARN'\"}}}]}"}'
