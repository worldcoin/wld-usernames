#!/bin/sh
# set -euo pipefail

# Create a queue named 'my-dev-queue'
awslocal sqs create-queue --queue-name wld-username-deletion-requests-local
