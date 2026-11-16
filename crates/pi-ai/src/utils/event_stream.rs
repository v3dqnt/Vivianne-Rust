//! Port of `src/utils/event-stream.ts`.
//!
//! The TS class is an `AsyncIterable` with a final-result promise. Here [`EventStream::next`] is the
//! iterator step and [`EventStream::result`] the final-result promise. One consumer iterates; the
//! producer calls `push` / `end` / `fail` from any task.

use std::collections::VecDeque;
use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::{watch, Notify};

use crate::error::flags::{classify_message, ClassifiableMessage};
use crate::error::provider::{ProviderResponseError, ProviderResponseErrorKind, ProviderResponseErrorOptions};
use crate::types::{AssistantMessage, AssistantMessageEvent, StopReason};

/// What a failed stream hands to its consumer and to `result()`.
pub type StreamFailure = Arc<dyn std::error::Error + Send + Sync>;

fn envelope_error(message: &str) -> StreamFailure {
    Arc::new(ProviderResponseError::new(
        message,
        ProviderResponseErrorOptions { kind: Some(ProviderResponseErrorKind::Envelope), ..Default::default() },
    ))
}

/// Anything a stream watchdog can consult for in-flight consumer-side local work.
pub trait LocalWorkSource: Send + Sync {
    fn has_pending_local_work(&self) -> bool;
    /// Epoch ms at which tracked local work last drained to zero, or 0 if none has completed. The
    /// provider cannot answer until it receives the local result, so idle watchdogs measure
    /// provider silence from this instant.
    fn local_work_settled_at(&self) -> u64;
}

/// Consumed head slots tolerated before the backlog is compacted. (The TS dequeues by advancing a
/// head index; a `VecDeque` pops in O(1), so there is no backlog to compact.)
pub const QUEUE_COMPACT_MIN_HEAD: usize = 64;

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64)
}

struct State<T> {
    /// Events pushed while no consumer was waiting.
    queue: VecDeque<T>,
    done: bool,
    /// True once the final result has been resolved or rejected.
    result_settled: bool,
    failure: Option<StreamFailure>,
    /// Consumer-side local operations currently in flight for this stream: a provider transport
    /// waiting on a server-requested local tool bridge (e.g. the Cursor exec channel) before it can
    /// send the result upstream. While non-zero, event silence is attributable to our own pending
    /// work, not a provider stall; idle watchdogs consult `has_pending_local_work`.
    pending_local_work: usize,
    local_work_settled_at: u64,
    /// A downstream stream whose local work also counts as ours: set when this stream forwards
    /// another stream's events (e.g. the Cursor discovered-id retry drains an inner stream), so the
    /// watchdog on this stream sees the inner exec bridge's busy state instead of aborting a
    /// healthy tool run.
    local_work_delegate: Option<Arc<dyn LocalWorkSource>>,
}

struct Inner<T, R> {
    state: Mutex<State<T>>,
    wake: Notify,
    result_tx: watch::Sender<Option<Result<R, StreamFailure>>>,
    is_complete: Box<dyn Fn(&T) -> bool + Send + Sync>,
    extract_result: Box<dyn Fn(&T) -> Result<R, StreamFailure> + Send + Sync>,
}

/// Generic event stream for async iteration.
pub struct EventStream<T, R = T> {
    inner: Arc<Inner<T, R>>,
}

impl<T, R> Clone for EventStream<T, R> {
    fn clone(&self) -> Self {
        Self { inner: self.inner.clone() }
    }
}

impl<T: Send + 'static, R: Clone + Send + Sync + 'static> EventStream<T, R> {
    pub fn new(
        is_complete: impl Fn(&T) -> bool + Send + Sync + 'static,
        extract_result: impl Fn(&T) -> Result<R, StreamFailure> + Send + Sync + 'static,
    ) -> Self {
        let (result_tx, _) = watch::channel(None);
        Self {
            inner: Arc::new(Inner {
                state: Mutex::new(State {
                    queue: VecDeque::new(),
                    done: false,
                    result_settled: false,
                    failure: None,
                    pending_local_work: 0,
                    local_work_settled_at: 0,
                    local_work_delegate: None,
                }),
                wake: Notify::new(),
                result_tx,
                is_complete: Box::new(is_complete),
                extract_result: Box::new(extract_result),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State<T>> {
        self.inner.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn settle(&self, value: Result<R, StreamFailure>) {
        // `send_replace` keeps the value for receivers created later, like a settled promise.
        self.inner.result_tx.send_replace(Some(value));
    }

    pub fn is_done(&self) -> bool {
        self.lock().done
    }

    pub fn result_settled(&self) -> bool {
        self.lock().result_settled
    }

    pub fn push(&self, event: T) {
        {
            let mut st = self.lock();
            if st.done {
                return;
            }
            if (self.inner.is_complete)(&event) {
                st.done = true;
                st.result_settled = true;
                let extracted = (self.inner.extract_result)(&event);
                drop(st);
                self.settle(extracted);
            }
        }
        self.deliver(event);
    }

    /// Hand an event to the consumer without the completion bookkeeping of [`push`](Self::push).
    pub fn deliver(&self, event: T) {
        self.lock().queue.push_back(event);
        self.inner.wake.notify_waiters();
        self.inner.wake.notify_one();
    }

    /// End the stream. With a result it settles `result()`; without one, `result()` rejects with a
    /// "Stream ended without a final result" envelope error unless it was already settled.
    pub fn end(&self, result: Option<R>) {
        let mut st = self.lock();
        st.done = true;
        match result {
            Some(result) => {
                st.result_settled = true;
                drop(st);
                self.settle(Ok(result));
            }
            None if !st.result_settled => {
                // end() without a terminal value must still settle result(): otherwise
                // complete()/result() awaits hang forever.
                st.result_settled = true;
                drop(st);
                self.settle(Err(envelope_error("Stream ended without a final result")));
            }
            None => drop(st),
        }
        // Notify all waiting consumers that we're done.
        self.end_waiting();
    }

    pub fn end_waiting(&self) {
        self.inner.wake.notify_waiters();
        self.inner.wake.notify_one();
    }

    pub fn fail(&self, err: StreamFailure) {
        {
            let mut st = self.lock();
            if st.done {
                return;
            }
            st.done = true;
            st.failure = Some(err.clone());
            st.result_settled = true;
        }
        self.settle(Err(err));
        self.inner.wake.notify_waiters();
        self.inner.wake.notify_one();
    }

    /// The next event, `Ok(None)` once the stream has ended, or the failure the producer reported
    /// (delivered once, after the queued events).
    pub async fn next(&self) -> Result<Option<T>, StreamFailure> {
        loop {
            let notified = self.inner.wake.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            {
                let mut st = self.lock();
                if let Some(event) = st.queue.pop_front() {
                    return Ok(Some(event));
                }
                if let Some(err) = st.failure.take() {
                    return Err(err);
                }
                if st.done {
                    return Ok(None);
                }
            }
            notified.await;
        }
    }

    /// The final result, once settled.
    pub async fn result(&self) -> Result<R, StreamFailure> {
        let mut rx = self.inner.result_tx.subscribe();
        loop {
            if let Some(v) = rx.borrow_and_update().clone() {
                return v;
            }
            if rx.changed().await.is_err() {
                return Err(envelope_error("Stream ended without a final result"));
            }
        }
    }

    /// True while local work tracked via [`track_local_work`](Self::track_local_work), on this
    /// stream or a forwarded delegate, is pending.
    pub fn has_pending_local_work(&self) -> bool {
        let st = self.lock();
        st.pending_local_work > 0 || st.local_work_delegate.as_ref().map_or(false, |d| d.has_pending_local_work())
    }

    /// Latest `local_work_settled_at` across this stream and a forwarded delegate.
    pub fn local_work_settled_at(&self) -> u64 {
        let st = self.lock();
        st.local_work_settled_at.max(st.local_work_delegate.as_ref().map_or(0, |d| d.local_work_settled_at()))
    }

    /// Count `source`'s pending local work as this stream's own. Used when this stream forwards
    /// another's events (Cursor discovered-id retry) so the watchdog does not abort a live tool run
    /// happening on the inner stream. Pass `None` to detach once forwarding ends.
    pub fn forward_local_work_from(&self, source: Option<Arc<dyn LocalWorkSource>>) {
        self.lock().local_work_delegate = source;
    }

    /// Track a local-work future so idle watchdogs on this stream do not treat the event silence
    /// while it is pending as a provider stall.
    pub async fn track_local_work<W>(&self, work: impl Future<Output = W>) -> W {
        self.lock().pending_local_work += 1;
        struct Guard<F: FnMut()>(F);
        impl<F: FnMut()> Drop for Guard<F> {
            fn drop(&mut self) {
                (self.0)();
            }
        }
        let _guard = Guard(|| {
            let mut st = self.lock();
            st.pending_local_work -= 1;
            if st.pending_local_work == 0 {
                st.local_work_settled_at = now_ms();
            }
        });
        work.await
    }
}

impl<T: Send + 'static, R: Clone + Send + Sync + 'static> LocalWorkSource for EventStream<T, R> {
    fn has_pending_local_work(&self) -> bool {
        EventStream::has_pending_local_work(self)
    }
    fn local_work_settled_at(&self) -> u64 {
        EventStream::local_work_settled_at(self)
    }
}

/// `AIError.classifyMessage(message)`: classify an errored assistant message and store the id on
/// it, as the TS does through `message.errorId = id`.
fn classify_error_message(message: &mut AssistantMessage) {
    let id = classify_message(&ClassifiableMessage {
        api: Some(&message.api),
        provider: Some(&message.provider),
        model: Some(&message.model),
        error_id: message.error_id.map(|v| v as u32),
        error_message: message.error_message.as_deref(),
        error_classification_message: message.error_classification_message.as_deref(),
        error_status: message.error_status.map(u32::from),
    });
    message.error_id = Some(u64::from(id));
}

/// The stream every provider returns: assistant-message events, resolving to the final message.
#[derive(Clone)]
pub struct AssistantMessageEventStream {
    stream: EventStream<AssistantMessageEvent, AssistantMessage>,
}

impl std::ops::Deref for AssistantMessageEventStream {
    type Target = EventStream<AssistantMessageEvent, AssistantMessage>;
    fn deref(&self) -> &Self::Target {
        &self.stream
    }
}

impl Default for AssistantMessageEventStream {
    fn default() -> Self {
        Self::new()
    }
}

impl AssistantMessageEventStream {
    pub fn new() -> Self {
        Self {
            stream: EventStream::new(
                |event: &AssistantMessageEvent| {
                    matches!(event, AssistantMessageEvent::Done { .. } | AssistantMessageEvent::Error { .. })
                },
                |event: &AssistantMessageEvent| match event {
                    AssistantMessageEvent::Done { message, .. } => Ok(message.clone()),
                    AssistantMessageEvent::Error { error, .. } => Ok(error.clone()),
                    _ => Err(envelope_error("Unexpected event type for final result")),
                },
            ),
        }
    }

    pub fn push(&self, mut event: AssistantMessageEvent) {
        if self.stream.is_done() {
            return;
        }
        if let AssistantMessageEvent::Error { error, .. } = &mut event {
            if error.stop_reason == StopReason::Error {
                classify_error_message(error);
            }
        }
        // Completion resolves the final result and still emits the terminal event.
        self.stream.push(event);
    }

    pub fn end(&self, mut result: Option<AssistantMessage>) {
        if let Some(r) = result.as_mut() {
            if r.stop_reason == StopReason::Error {
                classify_error_message(r);
            }
        }
        self.stream.end(result);
    }
}

/// Create an assistant-message event stream for legacy extension providers.
pub fn create_assistant_message_event_stream() -> AssistantMessageEventStream {
    AssistantMessageEventStream::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::flags::{is, Flag};
    use crate::types::{DoneReason, ErrorReason, Usage};

    fn message(stop: StopReason, error: Option<&str>) -> AssistantMessage {
        serde_json::from_value(serde_json::json!({
            "role": "assistant", "content": [], "api": "openai-completions", "provider": "openai", "model": "m",
            "usage": serde_json::to_value(Usage::default()).unwrap(),
            "stopReason": match stop { StopReason::Error => "error", StopReason::Stop => "stop", _ => "length" },
            "errorMessage": error, "timestamp": 1
        }))
        .unwrap()
    }

    #[tokio::test]
    async fn events_are_delivered_in_order_and_done_resolves_the_result() {
        let s = AssistantMessageEventStream::new();
        s.push(AssistantMessageEvent::Start { partial: message(StopReason::Stop, None) });
        s.push(AssistantMessageEvent::Done { reason: DoneReason::Stop, message: message(StopReason::Stop, None) });
        s.push(AssistantMessageEvent::Start { partial: message(StopReason::Stop, None) }); // ignored: done
        assert!(matches!(s.next().await.unwrap(), Some(AssistantMessageEvent::Start { .. })));
        assert!(matches!(s.next().await.unwrap(), Some(AssistantMessageEvent::Done { .. })));
        s.end(None);
        assert!(s.next().await.unwrap().is_none());
        assert_eq!(s.result().await.unwrap().stop_reason, StopReason::Stop);
    }

    #[tokio::test]
    async fn a_waiting_consumer_wakes_on_push() {
        let s = AssistantMessageEventStream::new();
        let s2 = s.clone();
        let consumer = tokio::spawn(async move { s2.next().await.unwrap() });
        tokio::task::yield_now().await;
        s.push(AssistantMessageEvent::Start { partial: message(StopReason::Stop, None) });
        assert!(matches!(consumer.await.unwrap(), Some(AssistantMessageEvent::Start { .. })));
    }

    #[tokio::test]
    async fn error_events_are_classified_before_delivery() {
        let s = AssistantMessageEventStream::new();
        s.push(AssistantMessageEvent::Error {
            reason: ErrorReason::Error,
            error: message(StopReason::Error, Some("503 service unavailable")),
        });
        let Some(AssistantMessageEvent::Error { error, .. }) = s.next().await.unwrap() else { panic!("expected error event") };
        assert!(is(error.error_id.map(|v| v as u32), Flag::Transient));
        assert!(is(s.result().await.unwrap().error_id.map(|v| v as u32), Flag::Transient));
    }

    #[tokio::test]
    async fn ending_without_a_result_rejects_it_and_fail_throws_after_the_queue() {
        let s = AssistantMessageEventStream::new();
        s.end(None);
        let err = s.result().await.unwrap_err();
        assert_eq!(err.to_string(), "Stream ended without a final result");

        let f = AssistantMessageEventStream::new();
        f.push(AssistantMessageEvent::Start { partial: message(StopReason::Stop, None) });
        f.fail(Arc::new(crate::error::abort::AbortError::default()));
        assert!(matches!(f.next().await.unwrap(), Some(AssistantMessageEvent::Start { .. })));
        assert_eq!(f.next().await.unwrap_err().to_string(), "Request was aborted");
        assert!(f.next().await.unwrap().is_none());
        assert_eq!(f.result().await.unwrap_err().to_string(), "Request was aborted");
    }

    #[tokio::test]
    async fn local_work_is_tracked_and_forwarded() {
        let s = AssistantMessageEventStream::new();
        assert!(!s.has_pending_local_work());
        let s2 = s.clone();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move { s2.track_local_work(async { rx.await.unwrap() }).await });
        tokio::task::yield_now().await;
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        assert!(s.has_pending_local_work());
        tx.send(()).unwrap();
        task.await.unwrap();
        assert!(!s.has_pending_local_work());
        assert!(s.local_work_settled_at() > 0);

        let outer = AssistantMessageEventStream::new();
        outer.forward_local_work_from(Some(Arc::new(s.stream.clone())));
        assert!(outer.local_work_settled_at() > 0);
    }
}
