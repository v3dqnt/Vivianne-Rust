//! What every call to the desktop goes through: a refusal to type a secret into an app, one retry of a call that
//! failed in a way the program calls safe to retry, and a cap on how much of a result reaches the model.

use std::future::Future;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

/// Most characters of one result that go into the model's context. A snapshot of a big app is the usual cause of more.
const MAX_RESULT_CHARS: usize = 40_000;

/// Which argument holds the text for the tools that put text into an app or the clipboard.
fn text_argument(tool: &str) -> Option<&'static str> {
    match tool {
        "desktop_type" => Some("text"),
        "desktop_set_value" => Some("value"),
        "desktop_clipboard_set" => Some("text"),
        _ => None,
    }
}

fn secret_patterns() -> &'static [Regex] {
    static PATTERNS: std::sync::OnceLock<Vec<Regex>> = std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            r"sk-[A-Za-z0-9_-]{16,}",
            r"gh[pousr]_[A-Za-z0-9]{20,}",
            r"github_pat_[A-Za-z0-9_]{20,}",
            r"AKIA[0-9A-Z]{16}",
            r"xox[abprs]-[A-Za-z0-9-]{10,}",
            r"AIza[0-9A-Za-z_-]{30,}",
            r"-----BEGIN [A-Z ]*PRIVATE KEY-----",
            r"eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            // A long unbroken run of key-like characters with no spaces.
            r"^[A-Za-z0-9+/=_-]{40,}$",
        ]
        .iter()
        .filter_map(|p| Regex::new(p).ok())
        .collect()
    })
}

/// Whether text looks like a key, token or private key.
#[must_use]
pub fn looks_like_secret(text: &str) -> bool {
    secret_patterns().iter().any(|p| p.is_match(text.trim()))
}

/// The reason to refuse, when the call would type something that looks like a key, token or password into an app.
/// What the agent types is shown to whatever app is in front, and may be sent anywhere; secrets are the user's to type.
#[must_use]
pub fn refuse_secret(tool: &str, args: &Value) -> Option<String> {
    let text = args.get(text_argument(tool)?)?.as_str()?;
    looks_like_secret(text).then(|| {
        "Refused: that text looks like a secret (a key, token or private key). Do not type secrets into apps: tell the user what is needed and let them type it themselves.".to_owned()
    })
}

/// Whether a result says the program failed in a way it calls safe to try again (its state file could not be
/// replaced, say), as opposed to a failure that retrying would repeat or an action that may have happened.
fn safe_to_retry(result: &Result<String, String>) -> bool {
    let (Ok(text) | Err(text)) = result;
    let Ok(json) = serde_json::from_str::<Value>(text) else {
        return false;
    };
    json["ok"] == Value::Bool(false)
        && json["error"]["code"] == "INTERNAL"
        && json["error"]["disposition"]["retry"] == "safe"
}

pub(crate) fn capped(text: String) -> String {
    if text.chars().count() <= MAX_RESULT_CHARS {
        return text;
    }
    let cut: String = text.chars().take(MAX_RESULT_CHARS).collect();
    format!(
        "{cut}\n… (the rest of this result was cut to save context: ask for less, for example a snapshot with `root` or a smaller `max_depth`)"
    )
}

/// Runs a desktop tool through `run` with the guard around it.
///
/// # Errors
/// The refusal, or whatever `run` failed with.
pub async fn call_guarded<F, Fut>(tool: &str, args: Value, run: F) -> Result<String, String>
where
    F: Fn(Value) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    if let Some(why) = refuse_secret(tool, &args) {
        return Err(why);
    }
    let mut result = run(args.clone()).await;
    if safe_to_retry(&result) {
        tokio::time::sleep(Duration::from_millis(400)).await;
        result = run(args).await;
    }
    result.map(capped).map_err(capped)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_secret_is_refused_but_ordinary_text_is_not() {
        for secret in [
            "sk-abcdefghijklmnopqrstuvwx",
            "ghp_abcdefghijklmnopqrstuvwxyz0123",
            "AKIAABCDEFGHIJKLMNOP",
            "-----BEGIN RSA PRIVATE KEY-----",
            "eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.abcdefghijk",
            "abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGH",
        ] {
            assert!(
                refuse_secret("desktop_type", &json!({"text": secret, "ref_id": "@s:e1"}))
                    .is_some(),
                "{secret}"
            );
        }
        for fine in [
            "meow",
            "Dear team, the build passed. Thanks!",
            "https://example.com/some/long/path/that/is/not/a/key",
            "5 + 3",
        ] {
            assert!(
                refuse_secret("desktop_type", &json!({"text": fine})).is_none(),
                "{fine}"
            );
        }
        // Only the tools that put text somewhere are checked, and by their own argument.
        assert!(
            refuse_secret(
                "desktop_set_value",
                &json!({"value": "sk-abcdefghijklmnopqrstuvwx"})
            )
            .is_some()
        );
        assert!(
            refuse_secret(
                "desktop_click",
                &json!({"text": "sk-abcdefghijklmnopqrstuvwx"})
            )
            .is_none()
        );
    }

    #[tokio::test]
    async fn a_failure_the_program_calls_safe_is_tried_once_more_and_one_that_is_not_is_not() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let calls = AtomicUsize::new(0);
        let flaky = r#"{"ok":false,"error":{"code":"INTERNAL","disposition":{"retry":"safe"}}}"#;
        let result = call_guarded("desktop_snapshot", json!({}), |_| {
            let n = calls.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    Ok(flaky.to_owned())
                } else {
                    Ok(r#"{"ok":true}"#.to_owned())
                }
            }
        })
        .await;
        assert_eq!(
            (result.as_deref(), calls.load(Ordering::SeqCst)),
            (Ok(r#"{"ok":true}"#), 2)
        );

        let calls = AtomicUsize::new(0);
        let unsafe_failure =
            r#"{"ok":false,"error":{"code":"INTERNAL","disposition":{"retry":"unsafe"}}}"#;
        let _ = call_guarded("desktop_click", json!({}), |_| {
            calls.fetch_add(1, Ordering::SeqCst);
            async move { Ok(unsafe_failure.to_owned()) }
        })
        .await;
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "an action that may have happened is never repeated"
        );
    }

    #[tokio::test]
    async fn a_huge_result_is_cut_and_says_how_to_ask_for_less() {
        let result = call_guarded("desktop_snapshot", json!({}), |_| async {
            Ok("x".repeat(MAX_RESULT_CHARS + 500))
        })
        .await
        .expect("ok");
        assert!(result.contains("was cut") && result.chars().count() < MAX_RESULT_CHARS + 300);
    }
}
