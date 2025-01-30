#!/bin/sh
awslocal sqs create-queue --queue-name wld-username-deletion-requests-local
awslocal sqs create-queue --queue-name wld-username-deletion-completion-local
