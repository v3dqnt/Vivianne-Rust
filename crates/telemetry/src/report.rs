//! Bug reports and the opt-in upload.
//!
//! **Nothing leaves this computer unless the user turned it on.** Upload needs both `telemetryEnabled: true`
//! and a `telemetryEndpoint` in settings (both off and empty by default), and the endpoint must be `https://`.
//! A report is always written to `~/.viv/agent/reports/` first, so what the user typed survives a failed or
//! skipped upload.

use std::future::Future;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use vivianne_ai::paths::write_json_atomic;

use crate::store::{Store, now_ms};

/// Events attached to a report: enough for context, not the whole history.
const REPORT_EVENT_TAIL: usize = 100;
/// How long an upload may take.
const UPLOAD_TIMEOUT: Duration = Duration::from_secs(15);

/// A written report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Where it was saved.
    pub path: PathBuf,
    /// It was also sent to the endpoint. `false` when upload is off, or when it failed (see `upload_error`).
    pub uploaded: bool,
    /// Why an upload that was tried did not work. `None` when it was not tried.
    pub upload_error: Option<String>,
}

/// Where a report may be sent: only when sharing is on **and** the endpoint is an `https://` address.
/// Anything else (a missing setting, `http://`, a blank) is "off", never "use a default".
#[must_use]
pub fn upload_target(settings: &Value) -> Option<String> {
    let enabled = settings
        .get("telemetryEnabled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let endpoint = settings
        .get("telemetryEndpoint")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let is_https = endpoint.len() > "https://".len()
        && endpoint
            .get(.."https://".len())
            .is_some_and(|scheme| scheme.eq_ignore_ascii_case("https://"));
    (enabled && is_https && !endpoint.contains(char::is_whitespace)).then(|| endpoint.to_owned())
}

/// POSTs `payload` to the endpoint in settings, if and only if the user has opted in and set a valid one.
/// `Ok(false)` means sending was skipped, so callers can tell "declined" from "failed".
pub async fn upload(payload: Value) -> Result<bool, String> {
    let settings = vivianne_ai::host::read_settings_json().await;
    let Some(endpoint) = upload_target(&settings) else {
        return Ok(false);
    };
    let response = vivianne_ai::ai::http::client()
        .post(endpoint)
        .json(&payload)
        .timeout(UPLOAD_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("upload failed: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("endpoint returned {}", response.status()));
    }
    Ok(true)
}

/// The report as JSON: what the user wrote, the build, and (if asked) recent events and the latest crash.
fn build(store: &Store, description: &str, include_diagnostics: bool, version: &str) -> Value {
    let (events, crashes) = if include_diagnostics {
        (store.read_events(), store.read_crashes())
    } else {
        (Vec::new(), Vec::new())
    };
    let tail = &events[events.len().saturating_sub(REPORT_EVENT_TAIL)..];
    json!({
        "ts": now_ms(),
        "description": description.trim(),
        "appVersion": version,
        "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "events": tail,
        "latestCrash": crashes.first(),
    })
}

/// Writes the report, then offers it to `send`. A failed send is reported in the result, never an error: the
/// report is already on disk.
pub(crate) async fn submit<F, Fut>(
    store: &Store,
    description: &str,
    include_diagnostics: bool,
    version: &str,
    send: F,
) -> Result<Report, String>
where
    F: FnOnce(Value) -> Fut,
    Fut: Future<Output = Result<bool, String>>,
{
    if description.trim().is_empty() {
        return Err("Please describe what went wrong.".to_owned());
    }
    let report = build(store, description, include_diagnostics, version);
    let path = store.dir().join("reports").join(format!(
        "report-{}.json",
        report["ts"].as_u64().unwrap_or(0)
    ));
    write_json_atomic(&path, &report).map_err(|e| format!("Could not save the report: {e}"))?;
    let (uploaded, upload_error) = match send(report).await {
        Ok(sent) => (sent, None),
        Err(why) => {
            log::warn!("[telemetry] report saved locally but upload failed: {why}");
            (false, Some(why))
        }
    };
    Ok(Report {
        path,
        uploaded,
        upload_error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store() -> Store {
        let dir = std::env::temp_dir().join(format!(
            "vivianne-report-{}-{}-{}",
            std::process::id(),
            now_ms(),
            format!("{:?}", std::thread::current().id()).replace(['(', ')'], "")
        ));
        Store::at(dir)
    }

    #[test]
    fn nothing_is_uploaded_without_both_settings_and_an_https_address() {
        let on =
            |endpoint: &str| json!({ "telemetryEnabled": true, "telemetryEndpoint": endpoint });
        assert_eq!(upload_target(&json!({})), None, "the default is off");
        assert_eq!(
            upload_target(&json!({ "telemetryEnabled": true })),
            None,
            "on, but nowhere to send"
        );
        assert_eq!(
            upload_target(&json!({ "telemetryEndpoint": "https://x.example/in" })),
            None,
            "an address, but not switched on"
        );
        assert_eq!(
            upload_target(
                &json!({ "telemetryEnabled": false, "telemetryEndpoint": "https://x.example/in" })
            ),
            None
        );
        assert_eq!(
            upload_target(&on("http://x.example/in")),
            None,
            "plain http is refused"
        );
        assert_eq!(upload_target(&on("ftp://x.example")), None);
        assert_eq!(upload_target(&on("https://")), None);
        assert_eq!(upload_target(&on("https://a b")), None);
        assert_eq!(
            upload_target(&on("  https://x.example/in ")),
            Some("https://x.example/in".to_owned())
        );
        assert_eq!(
            upload_target(&on("HTTPS://x.example/in")),
            Some("HTTPS://x.example/in".to_owned())
        );
    }

    #[tokio::test]
    async fn a_report_is_written_before_any_upload_and_survives_one_that_fails() {
        let store = temp_store();
        let report = submit(
            &store,
            "  it crashed when I opened settings ",
            false,
            "0.0.1",
            |_| async { Err("no route to host".to_owned()) },
        )
        .await
        .unwrap();
        assert!(!report.uploaded);
        assert_eq!(report.upload_error.as_deref(), Some("no route to host"));
        let saved: Value =
            serde_json::from_str(&std::fs::read_to_string(&report.path).unwrap()).unwrap();
        assert_eq!(saved["description"], "it crashed when I opened settings");
        assert_eq!(saved["appVersion"], "0.0.1");
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[tokio::test]
    async fn a_skipped_upload_is_not_an_error_and_a_sent_one_says_so() {
        let store = temp_store();
        let skipped = submit(&store, "x", false, "0.0.1", |_| async { Ok(false) })
            .await
            .unwrap();
        assert_eq!((skipped.uploaded, skipped.upload_error), (false, None));
        let sent = submit(&store, "y", false, "0.0.1", |_| async { Ok(true) })
            .await
            .unwrap();
        assert!(sent.uploaded);
        let _ = std::fs::remove_dir_all(store.dir());
    }

    #[tokio::test]
    async fn an_empty_description_writes_nothing() {
        let store = temp_store();
        let result = submit(&store, "   ", true, "0.0.1", |_| async { Ok(true) }).await;
        assert!(result.is_err());
        assert!(!store.dir().join("reports").exists());
    }

    #[tokio::test]
    async fn diagnostics_are_attached_only_when_asked() {
        let store = temp_store();
        store
            .track("app.start", &json!({ "os": "windows" }))
            .unwrap();
        let without = build(&store, "d", false, "0.0.1");
        assert_eq!(without["events"], json!([]));
        let with = build(&store, "d", true, "0.0.1");
        assert_eq!(with["events"].as_array().map(Vec::len), Some(1));
        let _ = std::fs::remove_dir_all(store.dir());
    }
}
