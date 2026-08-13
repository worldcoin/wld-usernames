use anyhow::Result;
use futures::{stream, StreamExt};
use tokio::{
	sync::broadcast,
	time::{sleep, Duration},
};
use tracing::{debug, error, info, instrument, warn};

use super::{
	deletion_completion_queue::{DataDeletionCompletion, DeletionCompletionQueue},
	deletion_request_queue::{DeletionRequestQueue, QueueMessage},
	username_deletion_service::UsernameDeletionService,
};

/// Bounded by the worker's dedicated DB pool (5 connections, see `mod.rs`).
const DEFAULT_MAX_CONCURRENCY: usize = 5;

/// Once shutdown is signalled, in-flight poll/batch work gets this long to
/// finish before it is abandoned. Must stay well below the deployment's stop
/// grace period (ECS default 30s) so a controlled abandon still beats the
/// SIGKILL that follows it.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(20);

/// Upper bound for one dependency operation (a single SQS receive, or one
/// message's delete → completion-send → acknowledge chain), so a hung
/// dependency cannot stall the worker indefinitely. Interrupted messages are
/// simply redelivered after the SQS visibility timeout (5 min).
const OPERATION_TIMEOUT: Duration = Duration::from_secs(30);

#[allow(clippy::module_name_repetitions)]
pub struct DataDeletionWorker {
	request_queue: Box<dyn DeletionRequestQueue>,
	completion_queue: Box<dyn DeletionCompletionQueue>,
	deletion_service: Box<dyn UsernameDeletionService>,
	sleep_interval: Duration,
	max_concurrency: usize,
	shutdown_grace: Duration,
	operation_timeout: Duration,
}

/// Wraps the shutdown channel so the signal is sticky: once observed by any
/// method, every later call reports it, even if the observing future was
/// dropped by a `select!`. A closed channel counts as shutdown, since the
/// sender only drops when the application is exiting.
struct ShutdownListener {
	receiver: broadcast::Receiver<()>,
	signaled: bool,
}

impl ShutdownListener {
	const fn new(receiver: broadcast::Receiver<()>) -> Self {
		Self {
			receiver,
			signaled: false,
		}
	}

	fn is_signaled(&mut self) -> bool {
		if !self.signaled {
			self.signaled = !matches!(
				self.receiver.try_recv(),
				Err(broadcast::error::TryRecvError::Empty)
			);
		}
		self.signaled
	}

	async fn wait(&mut self) {
		if !self.signaled {
			// No await point between the recv completing and the flag being
			// set, so a message can never be consumed and then lost to
			// cancellation.
			let _ = self.receiver.recv().await;
			self.signaled = true;
		}
	}

	async fn wait_then(&mut self, grace: Duration) {
		self.wait().await;
		sleep(grace).await;
	}
}

impl DataDeletionWorker {
	pub fn new(
		request_queue: Box<dyn DeletionRequestQueue>,
		completion_queue: Box<dyn DeletionCompletionQueue>,
		deletion_service: Box<dyn UsernameDeletionService>,
	) -> Result<Self> {
		let sleep_interval_secs = std::env::var("DELETION_WORKER_SLEEP_INTERVAL_SECS")?
			.parse::<u64>()
			.map_err(|e| anyhow::anyhow!("Invalid sleep interval: {}", e))?;

		let max_concurrency = match std::env::var("DELETION_WORKER_MAX_CONCURRENCY") {
			Ok(value) => value
				.parse::<usize>()
				.ok()
				.filter(|&concurrency| concurrency > 0)
				.ok_or_else(|| {
					anyhow::anyhow!("Invalid max concurrency (must be a positive integer): {value}")
				})?,
			Err(_) => DEFAULT_MAX_CONCURRENCY,
		};

		Ok(Self::with_config(
			request_queue,
			completion_queue,
			deletion_service,
			Duration::from_secs(sleep_interval_secs),
			max_concurrency,
			SHUTDOWN_GRACE,
			OPERATION_TIMEOUT,
		))
	}

	#[allow(clippy::too_many_arguments)]
	fn with_config(
		request_queue: Box<dyn DeletionRequestQueue>,
		completion_queue: Box<dyn DeletionCompletionQueue>,
		deletion_service: Box<dyn UsernameDeletionService>,
		sleep_interval: Duration,
		max_concurrency: usize,
		shutdown_grace: Duration,
		operation_timeout: Duration,
	) -> Self {
		Self {
			request_queue,
			completion_queue,
			deletion_service,
			sleep_interval,
			max_concurrency,
			shutdown_grace,
			operation_timeout,
		}
	}

	#[instrument(skip(self), err)]
	async fn handle_single_deletion(&self, deletion_request: QueueMessage) -> Result<()> {
		let message = deletion_request.request;

		debug!(correlation_id = %message.correlation_id, "Deleting username");

		self.deletion_service
			.delete_username(&message.user.wallet_address)
			.await?;

		info!(correlation_id = %message.correlation_id, "Deleted username");

		let completion_message = DataDeletionCompletion::new(message.correlation_id);
		self.completion_queue
			.send_message(completion_message)
			.await?;

		debug!(correlation_id = %message.correlation_id, "Sent completion message");

		self.request_queue
			.acknowledge(&deletion_request.receipt_handle)
			.await?;

		debug!(correlation_id = %message.correlation_id, "Acknowledged deletion request");

		Ok(())
	}

	/// Processes one batch of messages concurrently (bounded by
	/// `max_concurrency`), each under `operation_timeout`. Failed or timed-out
	/// messages are logged and left unacknowledged: SQS redelivers them after
	/// the visibility timeout and dead-letters them after `maxReceiveCount`.
	async fn process_batch(&self, deletion_requests: Vec<QueueMessage>) {
		let batch_size = deletion_requests.len();

		let failed = stream::iter(deletion_requests)
			.map(|deletion_request| async move {
				let correlation_id = deletion_request.request.correlation_id;
				let handled = tokio::time::timeout(
					self.operation_timeout,
					self.handle_single_deletion(deletion_request),
				)
				.await;
				match handled {
					Ok(Ok(())) => 0_usize,
					Ok(Err(e)) => {
						error!(
							correlation_id = %correlation_id,
							error = %e,
							error.kind = "username_deletion_failed",
							"Failed to delete username for {correlation_id}"
						);
						1
					},
					Err(_elapsed) => {
						error!(
							correlation_id = %correlation_id,
							error.kind = "username_deletion_timeout",
							"Deletion timed out after {}s for {correlation_id}, leaving message for redelivery",
							self.operation_timeout.as_secs()
						);
						1
					},
				}
			})
			.buffer_unordered(self.max_concurrency)
			.fold(0_usize, |acc, failures| async move { acc + failures })
			.await;

		info!(batch_size, failed, "Processed deletion batch");
	}

	/// One receive followed by processing of whatever it returned, both under
	/// `operation_timeout` bounds. Returns `true` if a non-empty batch was
	/// processed (the caller should poll again immediately), `false` if the
	/// queue was empty or the poll failed (the caller should idle first).
	async fn poll_and_process_once(&self) -> bool {
		let polled =
			tokio::time::timeout(self.operation_timeout, self.request_queue.poll_messages()).await;

		match polled {
			Ok(Ok(batch)) if !batch.is_empty() => {
				self.process_batch(batch).await;
				true
			},
			Ok(Ok(_)) => false,
			Ok(Err(e)) => {
				error!(
					error = %e,
					error.kind = "deletion_batch_poll_failed",
					"Error polling deletion requests: {e}"
				);
				false
			},
			Err(_elapsed) => {
				error!(
					error.kind = "deletion_batch_poll_timeout",
					"Polling deletion requests timed out after {}s",
					self.operation_timeout.as_secs()
				);
				false
			},
		}
	}

	/// Polls and processes batches back-to-back while the queue is non-empty,
	/// so a backlog drains at full speed instead of one batch per sleep
	/// interval. Sleeps for one interval when the queue is observed empty and
	/// after a poll error, so an SQS outage cannot turn into a tight retry
	/// loop.
	///
	/// Shutdown never cancels in-flight work outright: SQS dequeues messages
	/// server-side (visibility timeout started, receive count incremented)
	/// even if the client drops the response, so aborting a receive strands
	/// its messages until the visibility timeout, and aborting processing cuts
	/// a deletion off between its side effects. Instead, once shutdown is
	/// signalled, in-flight work gets `shutdown_grace` to finish and is then
	/// abandoned — equivalent to the SIGKILL that would otherwise follow, but
	/// logged and clean: unacknowledged messages are redelivered, deletions
	/// are idempotent, and completion events are keyed by correlation ID.
	/// Worst-case shutdown latency is therefore `shutdown_grace`, inside the
	/// deployment's stop grace period (ECS default 30s).
	pub async fn run(&self, shutdown: broadcast::Receiver<()>) {
		let mut shutdown = ShutdownListener::new(shutdown);

		info!(
			"Starting data deletion worker with {}s sleep interval and max concurrency {}...",
			self.sleep_interval.as_secs(),
			self.max_concurrency
		);

		loop {
			if shutdown.is_signaled() {
				break;
			}

			let completed = tokio::select! {
				had_messages = self.poll_and_process_once() => Some(had_messages),
				() = shutdown.wait_then(self.shutdown_grace) => None,
			};

			match completed {
				// In-flight work outlived the shutdown grace budget.
				None => {
					warn!(
						error.kind = "deletion_worker_shutdown_grace_exceeded",
						"In-flight deletion work did not finish within the {}s shutdown grace budget, abandoning; unacknowledged messages will be redelivered",
						self.shutdown_grace.as_secs()
					);
					break;
				},
				// Processed a non-empty batch: poll again immediately so a
				// backlog drains at full speed.
				Some(true) => {},
				// Empty queue or poll failure: idle for one interval. The
				// sleep is side-effect-free, so shutdown cancels it instantly.
				Some(false) => {
					tokio::select! {
						() = shutdown.wait() => break,
						() = sleep(self.sleep_interval) => {},
					}
				},
			}
		}

		info!("Shutdown signal received, data deletion worker stopped.");
	}
}

#[cfg(test)]
mod tests {
	use std::{
		collections::VecDeque,
		sync::{
			atomic::{AtomicUsize, Ordering},
			Arc, Mutex,
		},
	};

	use async_trait::async_trait;
	use tokio::sync::mpsc;
	use uuid::Uuid;

	use super::*;
	use crate::data_deletion_worker::{
		deletion_request_queue::{DataDeletionRequest, UserData},
		QueueError,
	};

	fn queue_message(wallet_address: &str) -> QueueMessage {
		QueueMessage {
			request: DataDeletionRequest {
				user: UserData {
					id: Uuid::new_v4(),
					public_key_id: "public-key-id".to_string(),
					wallet_address: wallet_address.to_string(),
				},
				correlation_id: Uuid::new_v4(),
				message_type: "account_deletion".to_string(),
				version: 1,
			},
			receipt_handle: format!("receipt-{wallet_address}"),
		}
	}

	#[derive(Default)]
	struct MockRequestQueueState {
		batches: Mutex<VecDeque<Result<Vec<QueueMessage>, QueueError>>>,
		polls: AtomicUsize,
		acknowledged: Mutex<Vec<String>>,
		/// When set, signals shutdown as soon as the scripted batches run out
		/// (an empty batch or an error is returned), so `run()` exits instead
		/// of idling in its sleep interval.
		shutdown_tx: Option<broadcast::Sender<()>>,
		/// When set, receives one event per poll as it starts, letting a test
		/// act (e.g. signal shutdown) while a receive is in flight.
		poll_started_tx: Option<mpsc::UnboundedSender<()>>,
		/// Delay between "SQS dequeued the messages server-side" (the pop) and
		/// the response reaching the client, to widen the in-flight window.
		poll_delay: Option<Duration>,
	}

	impl MockRequestQueueState {
		fn with_batches(batches: Vec<Result<Vec<QueueMessage>, QueueError>>) -> Self {
			Self {
				batches: Mutex::new(batches.into()),
				..Default::default()
			}
		}
	}

	struct MockRequestQueue(Arc<MockRequestQueueState>);

	#[async_trait]
	impl DeletionRequestQueue for MockRequestQueue {
		async fn poll_messages(&self) -> Result<Vec<QueueMessage>, QueueError> {
			self.0.polls.fetch_add(1, Ordering::SeqCst);

			if let Some(poll_started_tx) = &self.0.poll_started_tx {
				let _ = poll_started_tx.send(());
			}

			// The pop is the server-side dequeue; the delay is the response
			// still travelling back to the client.
			let next = self
				.0
				.batches
				.lock()
				.unwrap()
				.pop_front()
				.unwrap_or_else(|| Ok(vec![]));

			if let Some(delay) = self.0.poll_delay {
				sleep(delay).await;
			}

			if let Some(shutdown_tx) = &self.0.shutdown_tx {
				if !matches!(&next, Ok(batch) if !batch.is_empty()) {
					let _ = shutdown_tx.send(());
				}
			}

			next
		}

		async fn acknowledge(&self, receipt_handle: &str) -> Result<(), QueueError> {
			self.0
				.acknowledged
				.lock()
				.unwrap()
				.push(receipt_handle.to_string());
			Ok(())
		}
	}

	#[derive(Default)]
	struct MockCompletionQueueState {
		sent: Mutex<Vec<Uuid>>,
	}

	struct MockCompletionQueue(Arc<MockCompletionQueueState>);

	#[async_trait]
	impl DeletionCompletionQueue for MockCompletionQueue {
		async fn send_message(&self, completion: DataDeletionCompletion) -> Result<(), QueueError> {
			self.0.sent.lock().unwrap().push(completion.correlation_id);
			Ok(())
		}
	}

	#[derive(Default)]
	struct MockDeletionServiceState {
		deleted: Mutex<Vec<String>>,
		failing_wallet: Option<String>,
		delete_delay: Option<Duration>,
		/// When set, receives one event per deletion as it starts, letting a
		/// test act (e.g. signal shutdown) while a batch is mid-flight.
		started_tx: Option<mpsc::UnboundedSender<()>>,
		in_flight: AtomicUsize,
		max_in_flight: AtomicUsize,
	}

	struct MockDeletionService(Arc<MockDeletionServiceState>);

	#[async_trait]
	impl UsernameDeletionService for MockDeletionService {
		async fn delete_username(&self, wallet_address: &str) -> Result<(), QueueError> {
			if let Some(started_tx) = &self.0.started_tx {
				let _ = started_tx.send(());
			}

			let in_flight = self.0.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
			self.0.max_in_flight.fetch_max(in_flight, Ordering::SeqCst);

			if let Some(delay) = self.0.delete_delay {
				sleep(delay).await;
			}

			self.0.in_flight.fetch_sub(1, Ordering::SeqCst);

			if self.0.failing_wallet.as_deref() == Some(wallet_address) {
				return Err(QueueError::CacheInvalidationError(
					"mock failure".to_string(),
				));
			}

			self.0
				.deleted
				.lock()
				.unwrap()
				.push(wallet_address.to_string());
			Ok(())
		}
	}

	struct TestHarness {
		worker: DataDeletionWorker,
		request_queue: Arc<MockRequestQueueState>,
		completion_queue: Arc<MockCompletionQueueState>,
		deletion_service: Arc<MockDeletionServiceState>,
	}

	fn harness(
		request_queue: MockRequestQueueState,
		deletion_service: MockDeletionServiceState,
		max_concurrency: usize,
	) -> TestHarness {
		harness_with_timeouts(
			request_queue,
			deletion_service,
			max_concurrency,
			SHUTDOWN_GRACE,
			OPERATION_TIMEOUT,
		)
	}

	fn harness_with_timeouts(
		request_queue: MockRequestQueueState,
		deletion_service: MockDeletionServiceState,
		max_concurrency: usize,
		shutdown_grace: Duration,
		operation_timeout: Duration,
	) -> TestHarness {
		let request_queue = Arc::new(request_queue);
		let completion_queue = Arc::new(MockCompletionQueueState::default());
		let deletion_service = Arc::new(deletion_service);

		let worker = DataDeletionWorker::with_config(
			Box::new(MockRequestQueue(Arc::clone(&request_queue))),
			Box::new(MockCompletionQueue(Arc::clone(&completion_queue))),
			Box::new(MockDeletionService(Arc::clone(&deletion_service))),
			Duration::from_secs(60),
			max_concurrency,
			shutdown_grace,
			operation_timeout,
		);

		TestHarness {
			worker,
			request_queue,
			completion_queue,
			deletion_service,
		}
	}

	#[tokio::test]
	async fn run_drains_all_batches_back_to_back() {
		let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
		let harness = harness(
			MockRequestQueueState {
				shutdown_tx: Some(shutdown_tx),
				..MockRequestQueueState::with_batches(vec![
					Ok(vec![queue_message("0xaa"), queue_message("0xab")]),
					Ok(vec![queue_message("0xba"), queue_message("0xbb")]),
					Ok(vec![queue_message("0xca")]),
				])
			},
			MockDeletionServiceState::default(),
			DEFAULT_MAX_CONCURRENCY,
		);

		// The 5s timeout proves the batches are processed back-to-back: with
		// the old one-batch-per-interval loop this would take 60s+ per batch.
		tokio::time::timeout(Duration::from_secs(5), harness.worker.run(shutdown_rx))
			.await
			.expect("worker did not drain the backlog promptly");

		assert_eq!(harness.request_queue.polls.load(Ordering::SeqCst), 4);
		assert_eq!(harness.deletion_service.deleted.lock().unwrap().len(), 5);
		assert_eq!(harness.completion_queue.sent.lock().unwrap().len(), 5);
		assert_eq!(harness.request_queue.acknowledged.lock().unwrap().len(), 5);
	}

	#[tokio::test]
	async fn run_backs_off_after_poll_error_instead_of_retrying() {
		let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
		let harness = harness(
			MockRequestQueueState {
				shutdown_tx: Some(shutdown_tx),
				..MockRequestQueueState::with_batches(vec![
					Err(QueueError::InvalidMessage("mock poll failure".to_string())),
					Ok(vec![queue_message("0xaa")]),
				])
			},
			MockDeletionServiceState::default(),
			DEFAULT_MAX_CONCURRENCY,
		);

		tokio::time::timeout(Duration::from_secs(5), harness.worker.run(shutdown_rx))
			.await
			.expect("worker did not stop after shutdown signal");

		// The worker enters the backoff sleep after the error instead of
		// polling again immediately: the queued follow-up batch is never seen.
		assert_eq!(harness.request_queue.polls.load(Ordering::SeqCst), 1);
		assert!(harness.deletion_service.deleted.lock().unwrap().is_empty());
	}

	#[tokio::test]
	async fn failed_deletion_is_not_acknowledged_and_does_not_block_batch() {
		let failing = queue_message("0xbad");
		let failing_receipt = failing.receipt_handle.clone();
		let failing_correlation_id = failing.request.correlation_id;

		let harness = harness(
			MockRequestQueueState::default(),
			MockDeletionServiceState {
				failing_wallet: Some("0xbad".to_string()),
				..Default::default()
			},
			DEFAULT_MAX_CONCURRENCY,
		);

		harness
			.worker
			.process_batch(vec![queue_message("0xaa"), failing, queue_message("0xbb")])
			.await;

		assert_eq!(
			*harness.deletion_service.deleted.lock().unwrap(),
			vec!["0xaa".to_string(), "0xbb".to_string()]
		);

		let acknowledged = harness.request_queue.acknowledged.lock().unwrap().clone();
		assert_eq!(acknowledged.len(), 2);
		assert!(!acknowledged.contains(&failing_receipt));

		let sent = harness.completion_queue.sent.lock().unwrap().clone();
		assert_eq!(sent.len(), 2);
		assert!(!sent.contains(&failing_correlation_id));
	}

	#[tokio::test]
	async fn batch_processing_is_concurrent_but_bounded() {
		let batch = (0..10)
			.map(|i| queue_message(&format!("0x{i:02}")))
			.collect();

		let harness = harness(
			MockRequestQueueState::default(),
			MockDeletionServiceState {
				delete_delay: Some(Duration::from_millis(20)),
				..Default::default()
			},
			3,
		);

		harness.worker.process_batch(batch).await;

		let max_in_flight = harness
			.deletion_service
			.max_in_flight
			.load(Ordering::SeqCst);
		assert!(
			(2..=3).contains(&max_in_flight),
			"expected bounded concurrency, saw {max_in_flight} in flight"
		);
		assert_eq!(harness.deletion_service.deleted.lock().unwrap().len(), 10);
	}

	#[tokio::test]
	async fn run_stops_on_shutdown_signal() {
		let harness = harness(
			MockRequestQueueState::default(),
			MockDeletionServiceState::default(),
			1,
		);

		// The signal is queued before `run` starts, so the worker must exit on
		// its first pass through the loop instead of sleeping out the interval.
		let (shutdown_tx, shutdown_rx) = broadcast::channel(1);
		shutdown_tx.send(()).unwrap();

		tokio::time::timeout(Duration::from_secs(5), harness.worker.run(shutdown_rx))
			.await
			.expect("worker did not stop after shutdown signal");

		// Once shutdown is signalled, no further receive may be issued: each
		// receive has server-side effects (visibility timeout, receive count).
		assert_eq!(harness.request_queue.polls.load(Ordering::SeqCst), 0);
	}

	#[tokio::test]
	async fn shutdown_during_receive_does_not_abandon_received_batch() {
		let (poll_started_tx, mut poll_started_rx) = mpsc::unbounded_channel();
		let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

		let harness = harness(
			MockRequestQueueState {
				poll_started_tx: Some(poll_started_tx),
				poll_delay: Some(Duration::from_millis(50)),
				..MockRequestQueueState::with_batches(vec![Ok(vec![
					queue_message("0xaa"),
					queue_message("0xab"),
				])])
			},
			MockDeletionServiceState::default(),
			DEFAULT_MAX_CONCURRENCY,
		);

		// Signal shutdown while the receive is in flight: SQS has already
		// dequeued the messages server-side at this point, so cancelling the
		// receive would strand them until the visibility timeout.
		let signal_shutdown = async move {
			poll_started_rx.recv().await.expect("no poll ever started");
			shutdown_tx.send(()).expect("worker dropped the receiver");
		};

		tokio::time::timeout(
			Duration::from_secs(5),
			futures::future::join(harness.worker.run(shutdown_rx), signal_shutdown),
		)
		.await
		.expect("worker did not stop after shutdown signal");

		// The received batch must still be fully processed and acknowledged.
		assert_eq!(harness.deletion_service.deleted.lock().unwrap().len(), 2);
		assert_eq!(harness.completion_queue.sent.lock().unwrap().len(), 2);
		assert_eq!(harness.request_queue.acknowledged.lock().unwrap().len(), 2);
	}

	#[tokio::test]
	async fn shutdown_mid_batch_finishes_in_flight_deletions() {
		let (started_tx, mut started_rx) = mpsc::unbounded_channel();
		let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

		let harness = harness(
			MockRequestQueueState::with_batches(vec![Ok(vec![
				queue_message("0xaa"),
				queue_message("0xab"),
				queue_message("0xac"),
			])]),
			MockDeletionServiceState {
				delete_delay: Some(Duration::from_millis(50)),
				started_tx: Some(started_tx),
				..Default::default()
			},
			DEFAULT_MAX_CONCURRENCY,
		);

		// Signal shutdown as soon as the first deletion starts, while the
		// whole batch is still mid-flight.
		let signal_shutdown = async move {
			started_rx.recv().await.expect("no deletion ever started");
			shutdown_tx.send(()).expect("worker dropped the receiver");
		};

		tokio::time::timeout(
			Duration::from_secs(5),
			futures::future::join(harness.worker.run(shutdown_rx), signal_shutdown),
		)
		.await
		.expect("worker did not stop after shutdown signal");

		// Shutdown must not cancel deletions between their side effects: every
		// message in the received batch reaches acknowledgement.
		assert_eq!(harness.deletion_service.deleted.lock().unwrap().len(), 3);
		assert_eq!(harness.completion_queue.sent.lock().unwrap().len(), 3);
		assert_eq!(harness.request_queue.acknowledged.lock().unwrap().len(), 3);
	}

	#[tokio::test]
	async fn shutdown_abandons_work_stuck_beyond_grace_budget() {
		let (started_tx, mut started_rx) = mpsc::unbounded_channel();
		let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

		let harness = harness_with_timeouts(
			MockRequestQueueState::with_batches(vec![Ok(vec![queue_message("0xaa")])]),
			MockDeletionServiceState {
				// A dependency that effectively never resolves.
				delete_delay: Some(Duration::from_secs(3600)),
				started_tx: Some(started_tx),
				..Default::default()
			},
			DEFAULT_MAX_CONCURRENCY,
			Duration::from_millis(50),
			Duration::from_secs(3600),
		);

		let signal_shutdown = async move {
			started_rx.recv().await.expect("no deletion ever started");
			shutdown_tx.send(()).expect("worker dropped the receiver");
		};

		// Without the grace budget the stuck deletion would hold `run` for an
		// hour and ECS would SIGKILL the task instead.
		tokio::time::timeout(
			Duration::from_secs(5),
			futures::future::join(harness.worker.run(shutdown_rx), signal_shutdown),
		)
		.await
		.expect("worker did not abandon stuck work within the grace budget");

		// The stuck message is left unacknowledged for redelivery.
		assert!(harness.deletion_service.deleted.lock().unwrap().is_empty());
		assert!(harness.completion_queue.sent.lock().unwrap().is_empty());
		assert!(harness
			.request_queue
			.acknowledged
			.lock()
			.unwrap()
			.is_empty());
	}

	#[tokio::test]
	async fn stuck_deletion_times_out_and_leaves_message_for_redelivery() {
		let (shutdown_tx, shutdown_rx) = broadcast::channel(1);

		let harness = harness_with_timeouts(
			MockRequestQueueState {
				shutdown_tx: Some(shutdown_tx),
				..MockRequestQueueState::with_batches(vec![Ok(vec![queue_message("0xaa")])])
			},
			MockDeletionServiceState {
				// A dependency that effectively never resolves.
				delete_delay: Some(Duration::from_secs(3600)),
				..Default::default()
			},
			DEFAULT_MAX_CONCURRENCY,
			SHUTDOWN_GRACE,
			Duration::from_millis(50),
		);

		// Without the per-operation timeout the hung deletion would stall the
		// worker forever with no shutdown involved (the original "bricked
		// worker" failure mode). With it, the batch fails bounded, the worker
		// polls again (empty queue -> scripted shutdown), and the message
		// stays unacknowledged for redelivery.
		tokio::time::timeout(Duration::from_secs(5), harness.worker.run(shutdown_rx))
			.await
			.expect("worker stalled on a hung deletion instead of timing out");

		assert_eq!(harness.request_queue.polls.load(Ordering::SeqCst), 2);
		assert!(harness.deletion_service.deleted.lock().unwrap().is_empty());
		assert!(harness.completion_queue.sent.lock().unwrap().is_empty());
		assert!(harness
			.request_queue
			.acknowledged
			.lock()
			.unwrap()
			.is_empty());
	}
}
