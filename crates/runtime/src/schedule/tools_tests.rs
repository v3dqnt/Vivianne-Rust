//! The four tools, over a store in a temporary folder.

use std::path::PathBuf;

use serde_json::{Value, json};
use vivianne_core::{Mode, is_look_only};

use super::*;
use crate::orchestration::scope;

fn store() -> (PathBuf, Store) {
    let dir = std::env::temp_dir().join(format!("viv-schtool-{}", uuid::Uuid::new_v4().simple()));
    let store = Store::at(&dir.join("schedules.json"));
    (dir, store)
}

fn from(depth: usize) -> Caller {
    Caller {
        thread: "thread-1".into(),
        mode: Mode::Ask,
        folder: PathBuf::from("/project"),
        model: None,
        depth,
    }
}

fn body(result: &ToolResult) -> Value {
    serde_json::from_str(result.content.trim_start_matches("error:").trim()).expect("json")
}

#[tokio::test]
async fn create_defaults_to_this_conversation_and_list_view_edit_follow() {
    let (dir, store) = store();
    let create = ScheduleCreate(store.clone());
    let made = scope(from(0), async {
        create.execute(&json!({ "title": "Check", "prompt": "check ci", "every_seconds": 300 }), "").await
    })
    .await;
    assert!(!made.is_error, "{}", made.content);
    let id = body(&made)["id"].as_str().expect("id").to_owned();
    assert_eq!(body(&made)["target"]["session_id"], "thread-1");
    let listed = ScheduleList(store.clone()).execute(&json!({}), "").await;
    assert_eq!(body(&listed)["total"], 1);
    let viewed = ScheduleView(store.clone()).execute(&json!({ "id": id }), "").await;
    let stamp = body(&viewed)["updated_at"].as_i64().expect("stamp");
    let edit = ScheduleEdit(store.clone());
    let paused = edit
        .execute(&json!({ "id": id, "action": "update", "patch": { "active": false }, "expected_updated_at": stamp }), "")
        .await;
    assert_eq!(body(&paused)["active"], false);
    let stale = edit
        .execute(&json!({ "id": id, "action": "delete", "expected_updated_at": stamp }), "")
        .await;
    assert_eq!(body(&stale)["code"], "schedule_conflict");
    let gone = edit.execute(&json!({ "id": id, "action": "delete" }), "").await;
    assert!(!gone.is_error);
    let missing = ScheduleView(store).execute(&json!({ "id": id }), "").await;
    assert_eq!(body(&missing)["code"], "schedule_not_found");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn errors_carry_their_codes_and_a_subagent_cannot_schedule() {
    let (dir, store) = store();
    let create = ScheduleCreate(store.clone());
    let base = json!({ "title": "t", "prompt": "p" });
    let with = |extra: Value| {
        let mut args = base.clone();
        args.as_object_mut().expect("object").extend(extra.as_object().expect("object").clone());
        args
    };
    for (args, code) in [
        (with(json!({})), "invalid_selector"),
        (with(json!({ "after_seconds": 5, "every_seconds": 90 })), "invalid_selector"),
        (with(json!({ "every_seconds": 5 })), "frequency_too_high"),
        (with(json!({ "daily": { "time": "09:00:00", "time_zone": "Nowhere/City" } })), "invalid_time_zone"),
        (with(json!({ "at": "2000-01-01T00:00:00Z" })), "not_future"),
        (with(json!({ "after_seconds": 5, "target": "elsewhere" })), "invalid_selector"),
    ] {
        let got = create.execute(&args, "").await;
        assert_eq!(body(&got)["code"], code, "{args}");
    }
    let refused = scope(from(1), async { create.execute(&with(json!({ "after_seconds": 5 })), "").await }).await;
    assert_eq!(body(&refused)["code"], "subagent_session");
    assert_eq!(store.list(None, 0, 10).expect("listed").1, 0, "nothing was made");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_looking_tools_skip_approval() {
    assert!(is_look_only("schedule_list") && is_look_only("schedule_view"));
    assert!(!is_look_only("schedule_create") && !is_look_only("schedule_edit"));
}
