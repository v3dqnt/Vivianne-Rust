//! Port of `src/utils/abort.ts`.

use std::future::Future;
use std::sync::{Arc, Mutex};

use tokio_util::sync::CancellationToken;

use crate::error::abort::{caller_abort_reason, AbortError};

/// An `AbortSignal` with its `reason`: a cancellation token plus the reason recorded when it fired.
#[derive(Debug, Clone, Default)]
pub struct AbortSignal {
    token: CancellationToken,
    reason: Arc<Mutex<Option<String>>>,
}

impl AbortSignal {
    pub fn new() -> Self {
        Self::default()
    }

    /// `controller.abort(reason)`: the first call wins, as in JS.
    pub fn abort(&self, reason: Option<&str>) {
        if self.token.is_cancelled() {
            return;
        }
        *self.reason.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = reason.map(str::to_owned);
        self.token.cancel();
    }

    pub fn aborted(&self) -> bool {
        self.token.is_cancelled()
    }

    pub fn reason(&self) -> Option<String> {
        self.reason.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone()
    }

    pub async fn cancelled(&self) {
        self.token.cancelled().await;
    }

    pub fn token(&self) -> &CancellationToken {
        &self.token
    }
}

/// `AbortSignal.any([caller, request])`: fires when either source does.
#[derive(Debug, Clone)]
pub struct MergedSignal {
    caller: Option<AbortSignal>,
    request: AbortSignal,
}

impl MergedSignal {
    pub fn aborted(&self) -> bool {
        self.request.aborted() || self.caller.as_ref().map_or(false, AbortSignal::aborted)
    }

    pub async fn cancelled(&self) {
        match &self.caller {
            Some(caller) => {
                tokio::select! {
                    () = caller.cancelled() => {}
                    () = self.request.cancelled() => {}
                }
            }
            None => self.request.cancelled().await,
        }
    }
}

pub type LocalAbortReason = Arc<dyn std::error::Error + Send + Sync>;

/// Tracks whether a merged request signal was aborted by the caller or by provider-local logic.
///
/// Caller aborts always take priority. When both the caller and a local watchdog fire near each
/// other, the merged `requestSignal.reason` reflects whichever controller called `.abort()` first,
/// but ordering is racy and not meaningful for upstream consumers. What matters is intent: if the
/// caller's signal aborted, the request was cancelled by the caller, and any local watchdog reason
/// is incidental and **MUST NOT** be surfaced as a retryable transient error (which would cause
/// auto-retry to re-enter streaming and leave the UI showing a spinner the user already tried to
/// cancel).
#[derive(Debug)]
pub struct AbortSourceTracker {
    pub request_abort_controller: AbortSignal,
    pub request_signal: MergedSignal,
    caller_signal: Option<AbortSignal>,
    local_abort_reason: Mutex<Option<LocalAbortReasonSlot>>,
}

struct LocalAbortReasonSlot(LocalAbortReason);

impl std::fmt::Debug for LocalAbortReasonSlot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LocalAbortReason({})", self.0)
    }
}

pub fn create_abort_source_tracker(caller_signal: Option<AbortSignal>) -> AbortSourceTracker {
    let request_abort_controller = AbortSignal::new();
    let request_signal = MergedSignal { caller: caller_signal.clone(), request: request_abort_controller.clone() };
    AbortSourceTracker {
        request_abort_controller,
        request_signal,
        caller_signal,
        local_abort_reason: Mutex::new(None),
    }
}

impl AbortSourceTracker {
    pub fn abort_locally(&self, reason: LocalAbortReason) -> LocalAbortReason {
        *self.local_abort_reason.lock().unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(LocalAbortReasonSlot(reason.clone()));
        self.request_abort_controller.abort(Some(&reason.to_string()));
        reason
    }

    /// Caller intent dominates. Surface a local reason only when the caller did not abort, so
    /// timeout/idle-watchdog errors don't masquerade as the user's cancel.
    pub fn get_local_abort_reason(&self) -> Option<LocalAbortReason> {
        let local = self.local_abort_reason.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let slot = local.as_ref()?;
        if self.caller_signal.as_ref().map_or(false, AbortSignal::aborted) {
            return None;
        }
        Some(slot.0.clone())
    }

    /// The reason the caller passed to `abort()`, read from the caller's own signal, never the
    /// racy merged one.
    pub fn get_caller_abort_reason(&self) -> Option<String> {
        let caller = self.caller_signal.as_ref()?;
        caller_abort_reason(Some(caller.token()), caller.reason().as_deref())
    }

    /// If the caller signal aborted, treat it as a caller abort regardless of which controller won
    /// the race to set the merged reason. A heuristic on the merged reason flipped the result to
    /// `false` when a local watchdog fired microseconds before the user's ESC, which then routed
    /// user-initiated cancels through the auto-retry transient-error path.
    pub fn was_caller_abort(&self) -> bool {
        self.caller_signal.as_ref().map_or(false, AbortSignal::aborted)
    }
}

/// Race a shared future against a caller's signal without coupling the underlying work to that
/// signal. The future keeps running (and caches its result) even when an individual caller bails
/// out. When the signal aborts first, the signal's reason (or a default [`AbortError`]) is returned.
pub async fn race_with_signal<T>(
    promise: impl Future<Output = T>,
    signal: Option<&AbortSignal>,
) -> Result<T, AbortError> {
    let Some(signal) = signal else { return Ok(promise.await) };
    if signal.aborted() {
        return Err(AbortError::new(signal.reason().as_deref(), None));
    }
    tokio::select! {
        out = promise => Ok(out),
        () = signal.cancelled() => Err(AbortError::new(signal.reason().as_deref(), None)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caller_abort_dominates_a_local_reason() {
        let caller = AbortSignal::new();
        let tracker = create_abort_source_tracker(Some(caller.clone()));
        tracker.abort_locally(Arc::new(crate::error::validation::StreamTimeoutError::new(Some("idle"), None)).into_error());
        assert_eq!(tracker.get_local_abort_reason().map(|e| e.to_string()).as_deref(), Some("idle"));
        assert!(!tracker.was_caller_abort());
        caller.abort(Some("Interrupted by user"));
        assert!(tracker.was_caller_abort());
        assert!(tracker.get_local_abort_reason().is_none());
        assert_eq!(tracker.get_caller_abort_reason().as_deref(), Some("Interrupted by user"));
        assert!(tracker.request_signal.aborted());
    }

    #[test]
    fn abort_keeps_the_first_reason() {
        let s = AbortSignal::new();
        s.abort(Some("a"));
        s.abort(Some("b"));
        assert_eq!(s.reason().as_deref(), Some("a"));
    }

    #[tokio::test]
    async fn race_returns_the_value_or_the_abort() {
        assert_eq!(race_with_signal(async { 5 }, None).await.unwrap(), 5);
        let s = AbortSignal::new();
        s.abort(Some("stop"));
        let err = race_with_signal(std::future::pending::<()>(), Some(&s)).await.unwrap_err();
        assert_eq!(err.to_string(), "stop");
        let live = AbortSignal::new();
        assert_eq!(race_with_signal(async { "ok" }, Some(&live)).await.unwrap(), "ok");
        let default = AbortSignal::new();
        default.abort(None);
        assert_eq!(race_with_signal(async { 1 }, Some(&default)).await.unwrap_err().to_string(), "Request was aborted");
    }

    trait IntoError {
        fn into_error(self) -> LocalAbortReason;
    }
    impl IntoError for Arc<crate::error::validation::StreamTimeoutError> {
        fn into_error(self) -> LocalAbortReason {
            self
        }
    }
}
