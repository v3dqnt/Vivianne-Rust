//! Port of `src/error/format.ts`.

use super::flags::{llama_cpp_tool_call_parse_pattern, ErrorLike};
use crate::utils::http_inspector::{
    finalize_error_message, rewrite_cline_pass_error, rewrite_copilot_error, CapturedHttpErrorResponse, DumpEnvironment,
    RawHttpRequestDump,
};
use crate::utils::retry_after::format_error_message_with_retry_after;

fn rewrite_ollama_tool_call_json_error(message: &str) -> String {
    if !llama_cpp_tool_call_parse_pattern().is_match(message) {
        return message.to_owned();
    }
    format!("Local Ollama model emitted malformed tool-call JSON and llama.cpp rejected it (HTTP 500). This is usually a deterministic model-output failure after context degradation, not a transient server outage; reload the model or reduce context, then retry.\n{message}")
}

/// Inputs that steer [`format_message`]'s formatter selection.
#[derive(Debug, Clone, Default)]
pub struct FormatMessageOptions<'a> {
    /// When present, the raw request is dumped into the message for 400-class failures.
    pub raw_request_dump: Option<&'a RawHttpRequestDump>,
    /// Captured non-2xx response body, appended to the message when available.
    pub captured_error_response: Option<&'a CapturedHttpErrorResponse>,
    /// Provider id; gates provider-specific user-facing rewrites.
    pub provider: Option<&'a str>,
}

/// Format a provider error into a user-facing message, unifying the three formatters: lightweight
/// retry-after extraction, the raw-dump finalizer, and the copilot rewrite.
///
/// Selection is driven by inputs, not a mode flag: a `raw_request_dump` routes through
/// [`finalize_error_message`] (retry-after + raw dump + captured body), otherwise the lightweight
/// [`format_error_message_with_retry_after`] is used.
pub async fn format_message(error: &dyn ErrorLike, opts: &FormatMessageOptions<'_>, env: &DumpEnvironment) -> String {
    let mut message = match opts.raw_request_dump {
        Some(dump) => finalize_error_message(error, Some(dump), opts.captured_error_response, env).await,
        None => format_error_message_with_retry_after(error),
    };
    if opts.provider == Some("github-copilot") {
        message = rewrite_copilot_error(&message, error, "github-copilot");
    }
    if opts.provider == Some("cline-pass") {
        message = rewrite_cline_pass_error(&message, "cline-pass");
    }
    if opts.provider == Some("ollama") {
        message = rewrite_ollama_tool_call_json_error(&message);
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::classes::{ProviderHttpError, ProviderHttpErrorOptions};
    use reqwest::header::HeaderMap;

    fn env() -> DumpEnvironment {
        DumpEnvironment { logs_dir: std::env::temp_dir(), is_test_runtime: true }
    }

    #[tokio::test]
    async fn lightweight_path_appends_the_retry_hint() {
        let mut headers = HeaderMap::new();
        headers.insert("retry-after", "2".parse().unwrap());
        let e = ProviderHttpError::new("429 slow", 429, ProviderHttpErrorOptions { headers: Some(headers), ..Default::default() });
        let msg = format_message(&e, &FormatMessageOptions::default(), &env()).await;
        assert_eq!(msg, "429 slow retry-after-ms=2000");
    }

    #[tokio::test]
    async fn provider_rewrites_apply_after_formatting() {
        let e = ProviderHttpError::new("401 nope", 401, ProviderHttpErrorOptions::default());
        let opts = FormatMessageOptions { provider: Some("github-copilot"), ..Default::default() };
        assert!(format_message(&e, &opts, &env()).await.starts_with("GitHub Copilot authentication failed (HTTP 401)"));
        let ollama = ProviderHttpError::new("500 failed to parse tool call arguments as json", 500, ProviderHttpErrorOptions::default());
        let opts = FormatMessageOptions { provider: Some("ollama"), ..Default::default() };
        let msg = format_message(&ollama, &opts, &env()).await;
        assert!(msg.starts_with("Local Ollama model emitted malformed tool-call JSON"));
        assert!(msg.ends_with("failed to parse tool call arguments as json"));
    }
}
