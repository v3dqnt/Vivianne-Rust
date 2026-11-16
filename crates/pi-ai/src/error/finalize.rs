//! Port of `src/error/finalize.ts`.

use super::abort::caller_abort_reason;
use super::flags::{classify, classify_message, status, ClassifiableMessage, ErrorLike};
use super::format::{format_message, FormatMessageOptions};
use crate::utils::abort::{AbortSignal, AbortSourceTracker};
use crate::utils::http_inspector::{CapturedHttpErrorResponse, DumpEnvironment, RawHttpRequestDump};

/// Context a provider catch block hands to [`finalize`].
#[derive(Default)]
pub struct FinalizeOptions<'a> {
    /// Wire API, for api-specific text classification (e.g. stale-responses items).
    pub api: Option<&'a str>,
    /// Provider id; forwarded to the message formatter for copilot rewrites.
    pub provider: Option<&'a str>,
    /// Requested model id; paired with provider for model-entitlement classification.
    pub model: Option<&'a str>,
    /// Caller signal, for providers that don't run an abort tracker.
    pub signal: Option<&'a AbortSignal>,
    /// Abort tracker, preferred over `signal`: distinguishes caller vs. local aborts.
    pub abort_tracker: Option<&'a AbortSourceTracker>,
    /// Raw request, dumped into the message for 400-class failures.
    pub raw_request_dump: Option<&'a RawHttpRequestDump>,
    /// Captured non-2xx response body, used for status fallback and message detail.
    pub captured_error_response: Option<&'a CapturedHttpErrorResponse>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinalizedStopReason {
    Aborted,
    Error,
}

/// The full bundle a provider assigns onto its `AssistantMessage` error fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizeResult {
    /// Structured flag id from [`classify`].
    pub id: u32,
    /// HTTP status, from the error or the captured response.
    pub status: Option<u32>,
    /// `Aborted` when the caller cancelled, otherwise `Error`.
    pub stop_reason: FinalizedStopReason,
    /// User-facing message: a local abort reason, else the caller's abort reason, else from [`format_message`].
    pub message: String,
}

/// Wraps an error so the captured response's status sits at the root of the cause chain.
struct WithCapturedStatus<'a> {
    status: u32,
    cause: &'a dyn ErrorLike,
}

impl ErrorLike for WithCapturedStatus<'_> {
    fn own_status(&self) -> Option<u32> {
        Some(self.status)
    }
    fn cause(&self) -> Option<&dyn ErrorLike> {
        Some(self.cause)
    }
}

/// Build the complete error bundle for a provider catch block, replacing the
/// `stopReason` / `errorStatus` / `errorId` / `errorMessage` boilerplate.
///
/// `stop_reason` comes from the abort tracker (caller intent dominates) or, when no tracker is
/// supplied, the raw `signal.aborted`. The message is, in order, a local abort reason (e.g. a
/// first-event timeout), the reason the caller passed to `abort()` (e.g. "Interrupted by user"),
/// or the formatted error.
pub async fn finalize(error: &dyn ErrorLike, opts: &FinalizeOptions<'_>, env: &DumpEnvironment) -> FinalizeResult {
    let aborted = match opts.abort_tracker {
        Some(tracker) => tracker.was_caller_abort(),
        None => opts.signal.map_or(false, AbortSignal::aborted),
    };
    let error_status = status(error);
    let current_status = error_status.or_else(|| opts.captured_error_response.map(|c| c.status));

    let message = {
        let local_reason = opts.abort_tracker.and_then(AbortSourceTracker::get_local_abort_reason);
        let caller_reason = match opts.abort_tracker {
            Some(tracker) => tracker.get_caller_abort_reason(),
            None => opts.signal.and_then(|s| caller_abort_reason(Some(s.token()), s.reason().as_deref())),
        };
        match (local_reason, caller_reason) {
            (Some(local), _) => local.to_string(),
            (None, Some(caller)) => caller,
            (None, None) => {
                let fmt_opts = FormatMessageOptions {
                    raw_request_dump: opts.raw_request_dump,
                    captured_error_response: opts.captured_error_response,
                    provider: opts.provider,
                };
                format_message(error, &fmt_opts, env).await
            }
        }
    };

    // A captured status is transport context for the original error. Put it at the root of the
    // classification cause chain so terminal 4xx policy governs nested diagnostics before they
    // can contribute transient flags.
    let id = match (error_status, current_status) {
        (None, Some(captured_status)) => {
            let wrapped = WithCapturedStatus { status: captured_status, cause: error };
            classify_with_message(&wrapped, &message, opts, current_status)
        }
        _ => classify_with_message(error, &message, opts, current_status),
    };

    FinalizeResult {
        id,
        status: current_status,
        stop_reason: if aborted { FinalizedStopReason::Aborted } else { FinalizedStopReason::Error },
        message,
    }
}

fn classify_with_message(
    classification_error: &dyn ErrorLike,
    message: &str,
    opts: &FinalizeOptions<'_>,
    current_status: Option<u32>,
) -> u32 {
    classify_message(&ClassifiableMessage {
        api: opts.api,
        provider: opts.provider,
        model: opts.model,
        error_id: Some(classify(classification_error, opts.api)),
        error_message: Some(message),
        error_classification_message: None,
        error_status: current_status,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::classes::{ProviderHttpError, ProviderHttpErrorOptions};
    use crate::error::flags::{is, Flag, TextError};
    use crate::utils::abort::create_abort_source_tracker;

    fn env() -> DumpEnvironment {
        DumpEnvironment { logs_dir: std::env::temp_dir(), is_test_runtime: true }
    }

    #[tokio::test]
    async fn http_error_is_classified_and_keeps_its_status() {
        let e = ProviderHttpError::new("503 overloaded", 503, ProviderHttpErrorOptions::default());
        let r = finalize(&e, &FinalizeOptions::default(), &env()).await;
        assert_eq!(r.status, Some(503));
        assert_eq!(r.stop_reason, FinalizedStopReason::Error);
        assert_eq!(r.message, "503 overloaded");
        assert!(is(Some(r.id), Flag::Transient));
    }

    #[tokio::test]
    async fn caller_abort_wins_and_reports_the_callers_reason() {
        let caller = AbortSignal::new();
        let tracker = create_abort_source_tracker(Some(caller.clone()));
        caller.abort(Some("Interrupted by user"));
        let opts = FinalizeOptions { abort_tracker: Some(&tracker), ..Default::default() };
        let r = finalize(&TextError("socket hang up"), &opts, &env()).await;
        assert_eq!(r.stop_reason, FinalizedStopReason::Aborted);
        assert_eq!(r.message, "Interrupted by user");
    }

    #[tokio::test]
    async fn local_abort_reason_becomes_the_message() {
        let tracker = create_abort_source_tracker(None);
        tracker.abort_locally(std::sync::Arc::new(crate::error::validation::StreamTimeoutError::new(Some("first event timeout"), None)));
        let opts = FinalizeOptions { abort_tracker: Some(&tracker), ..Default::default() };
        let r = finalize(&TextError("x"), &opts, &env()).await;
        assert_eq!(r.message, "first event timeout");
        assert_eq!(r.stop_reason, FinalizedStopReason::Error);
    }

    #[tokio::test]
    async fn captured_status_governs_a_statusless_error() {
        let captured = CapturedHttpErrorResponse { status: 400, ..Default::default() };
        let opts = FinalizeOptions { captured_error_response: Some(&captured), ..Default::default() };
        let r = finalize(&TextError("unexpected EOF"), &opts, &env()).await;
        assert_eq!(r.status, Some(400));
        assert!(!is(Some(r.id), Flag::Transient));
    }
}
