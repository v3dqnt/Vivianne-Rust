use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Value, json};
use vivianne_core::{Mode, ToolCall};
use vivianne_providers::{ChatEvent, ChatRequest, EventStream, Provider, ProviderError};
use vivianne_tools::{Tool, Toolbox};

use super::board::{Board, Owner};
use super::*;
use crate::Agent;
use crate::orchestration::{AgentSpec, Caller, Spawner, Start, scope, start};

fn folder(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viv-team-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn lead() -> Actor {
    Actor {
        owner: Owner {
            id: "lead".into(),
            name: "lead".into(),
        },
        lead: true,
    }
}

fn mate(name: &str) -> Actor {
    Actor {
        owner: Owner {
            id: format!("id-{name}"),
            name: name.into(),
        },
        lead: false,
    }
}

fn add(board: &mut Board, subject: &str, blocked_by: &[&str], scopes: &[&str]) -> View {
    board
        .create(&Create {
            subject: subject.into(),
            description: "do it".into(),
            blocked_by: blocked_by.iter().map(|s| (*s).to_owned()).collect(),
            write_scopes: scopes.iter().map(|s| (*s).to_owned()).collect(),
        })
        .expect("created")
}

fn code(result: Result<View, TeamError>) -> &'static str {
    result.expect_err("refused").code
}

#[test]
fn revisions_ownership_dependencies_and_transitions_are_enforced() {
    let mut board = Board::default();
    let first = add(&mut board, "first", &[], &[]);
    let second = add(&mut board, "second", &["task-1"], &[]);
    assert!(first.ready && !second.ready);
    let a = mate("a");
    let act = |board: &mut Board, who: &Actor, id: &str, rev: u64, action: Action| {
        board.update(who, &Update::new(id, rev, action))
    };
    assert_eq!(
        code(act(&mut board, &a, "task-2", 1, Action::Claim)),
        "TEAM_TASK_BLOCKED"
    );
    let claimed = act(&mut board, &a, "task-1", 1, Action::Claim).expect("claimed");
    assert_eq!((claimed.revision, claimed.owner.as_deref()), (2, Some("a")));
    assert_eq!(
        code(act(&mut board, &a, "task-1", 1, Action::Complete)),
        "TEAM_TASK_STALE_REVISION"
    );
    assert_eq!(
        code(act(&mut board, &mate("b"), "task-1", 2, Action::Complete)),
        "TEAM_TASK_UNAUTHORIZED"
    );
    assert_eq!(
        code(act(&mut board, &mate("b"), "task-1", 2, Action::Claim)),
        "TEAM_TASK_ALREADY_CLAIMED"
    );
    assert_eq!(
        code(act(&mut board, &a, "task-1", 2, Action::Reassign)),
        "TEAM_LEAD_REQUIRED"
    );
    assert_eq!(
        code(act(&mut board, &lead(), "task-1", 2, Action::Delete)),
        "TEAM_TASK_HAS_DEPENDENTS"
    );
    act(&mut board, &a, "task-1", 2, Action::Complete).expect("done");
    assert!(board.get("task-2").unwrap().ready);
    assert_eq!(
        code(act(&mut board, &a, "task-1", 3, Action::Release)),
        "TEAM_TASK_INVALID_TRANSITION"
    );
    let reopened = act(&mut board, &lead(), "task-1", 3, Action::Reopen).expect("reopened");
    assert_eq!(reopened.owner, None);
    assert_eq!(
        code(act(&mut board, &a, "task-9", 1, Action::Claim)),
        "TEAM_TASK_NOT_FOUND"
    );
}

#[test]
fn cycles_bad_scopes_and_bad_text_are_refused_and_overlaps_only_warn() {
    let mut board = Board::default();
    add(&mut board, "one", &[], &["src/a"]);
    add(&mut board, "two", &["task-1"], &["src/a/b.rs", "./docs/"]);
    let mut change = Update::new("task-1", 1, Action::SetDependencies);
    change.blocked_by = Some(vec!["task-2".into()]);
    assert_eq!(
        code(board.update(&lead(), &change)),
        "TEAM_TASK_DEPENDENCY_CYCLE"
    );
    change.blocked_by = Some(vec!["task-1".into()]);
    assert_eq!(
        code(board.update(&lead(), &change)),
        "TEAM_TASK_DEPENDENCY_CYCLE"
    );
    assert_eq!(
        board.get("task-2").unwrap().write_scopes,
        ["src/a/b.rs", "docs"]
    );
    board
        .update(&lead(), &Update::new("task-1", 1, Action::Claim))
        .unwrap();
    assert_eq!(
        board.get("task-2").unwrap().write_scope_warnings,
        ["write scopes overlap with task-1"]
    );
    for scope in ["", "/etc", "C:\\x", "a/../b", "a//b"] {
        let made = board.create(&Create {
            subject: "s".into(),
            description: "d".into(),
            blocked_by: Vec::new(),
            write_scopes: vec![scope.into()],
        });
        assert_eq!(code(made), "TEAM_INVALID_WRITE_SCOPE", "{scope:?}");
    }
    let blank = board.create(&Create {
        subject: " ".into(),
        description: "d".into(),
        ..Create::default()
    });
    assert_eq!(code(blank), "TEAM_INVALID_ARGUMENT");
}

#[test]
fn the_board_is_read_back_and_a_deleted_number_is_not_used_again() {
    let dir = folder("store");
    let store = Store::new(dir.clone());
    store
        .change("t-1", |b| Ok(add(b, "gone", &[], &[])))
        .unwrap();
    store
        .change("t-1", |b| {
            b.update(&lead(), &Update::new("task-1", 1, Action::Delete))
        })
        .unwrap();
    let again = Store::new(dir.clone());
    assert!(again.read("t-1").unwrap().list().is_empty());
    assert_eq!(
        again
            .change("t-1", |b| Ok(add(b, "next", &[], &[])))
            .unwrap()
            .id,
        "task-2"
    );
    assert!(
        again.read("../t").is_err(),
        "an id cannot name another folder"
    );
    std::fs::write(dir.join("t-1.json"), "not json").unwrap();
    assert!(again.change("t-1", |b| Ok(add(b, "x", &[], &[]))).is_err());
    assert_eq!(
        std::fs::read_to_string(dir.join("t-1.json")).unwrap(),
        "not json",
        "left as it was"
    );
    let _ = std::fs::remove_dir_all(dir);
}

struct Scripted(Mutex<VecDeque<Vec<ChatEvent>>>);

#[async_trait]
impl Provider for Scripted {
    fn id(&self) -> &'static str {
        "scripted"
    }

    async fn stream(&self, _request: &ChatRequest) -> Result<EventStream, ProviderError> {
        let events = self
            .0
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(vec![ChatEvent::Done]);
        Ok(Box::pin(futures::stream::iter(events.into_iter().map(Ok))))
    }
}

/// A subagent that claims task 1 and completes it, with only the board's tools.
struct Claims(Store);

impl Spawner for Claims {
    fn build(&self, spec: &AgentSpec) -> Result<Agent, String> {
        let step = |id: &str, revision: u64, action: &str| {
            vec![
                ChatEvent::ToolCall(ToolCall {
                    id: id.to_owned(),
                    name: "team_task_update".to_owned(),
                    args: json!({ "task_id": "task-1", "expected_revision": revision, "action": action }),
                }),
                ChatEvent::Done,
            ]
        };
        let replies = VecDeque::from([step("c1", 1, "claim"), step("c2", 2, "complete")]);
        let mut tools = Toolbox::new(&spec.folder);
        for kind in Kind::ALL {
            tools.add(TeamTool(kind, self.0.clone()));
        }
        let agent = Agent::new(Arc::new(Scripted(Mutex::new(replies))), "m", None, tools);
        agent.set_mode(spec.mode);
        Ok(agent)
    }
}

fn caller(thread: &str) -> Caller {
    Caller {
        thread: thread.to_owned(),
        mode: Mode::Ask,
        folder: std::env::temp_dir(),
        model: None,
        provider: None,
        confined: false,
        depth: 0,
        member: None,
    }
}

async fn call(store: &Store, caller: &Caller, kind: Kind, args: Value) -> Value {
    let tool = TeamTool(kind, store.clone());
    let out = scope(caller.clone(), tool.execute(&args, "")).await;
    assert!(!out.is_error, "{}", out.content);
    serde_json::from_str(&out.content).unwrap()
}

/// The deciding case: in Ask mode a subagent is read-only and nobody answers it, and it still works the board.
#[tokio::test]
async fn a_read_only_subagent_claims_and_completes_a_task_the_lead_made() {
    let dir = folder("member");
    let store = Store::new(dir.clone());
    let lead = caller("team-member");
    call(
        &store,
        &lead,
        Kind::Create,
        json!({ "subject": "s", "description": "d" }),
    )
    .await;
    let handle = start(
        &Claims(store.clone()),
        &lead,
        Start {
            name: "reviewer".into(),
            description: "reviews".into(),
            profile: "explore".into(),
            prompt: "claim task-1".into(),
            spec: AgentSpec {
                folder: std::env::temp_dir(),
                mode: Mode::Plan,
                system: String::new(),
                model: None,
                provider: None,
            },
        },
    )
    .expect("started");
    handle.wait(Duration::from_secs(10)).await;
    let task = call(&store, &lead, Kind::Get, json!({ "task_id": "task-1" })).await;
    assert_eq!(
        (task["status"].as_str(), task["owner"].as_str()),
        (Some("completed"), Some("reviewer")),
        "{task}"
    );
    let listed = call(&store, &lead, Kind::List, json!({ "owner": "reviewer" })).await;
    assert_eq!(listed["tasks"].as_array().map(Vec::len), Some(1));
    // It has ended, so nothing more can be handed to it.
    let tool = TeamTool(Kind::Update, store.clone());
    call(
        &store,
        &lead,
        Kind::Create,
        json!({ "subject": "s2", "description": "d" }),
    )
    .await;
    let args = json!({ "task_id": "task-2", "expected_revision": 1, "action": "reassign", "owner": "reviewer" });
    let out = scope(lead.clone(), tool.execute(&args, "")).await;
    assert!(
        out.content.contains("TEAM_MEMBER_NOT_FOUND"),
        "{}",
        out.content
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_boards_tools_run_without_asking_and_in_plan_mode_but_never_side_by_side() {
    for name in [
        "team_task_create",
        "team_task_list",
        "team_task_get",
        "team_task_update",
    ] {
        assert!(
            Mode::Plan.allows(name) && !Mode::Ask.needs_approval(name),
            "{name}"
        );
        assert!(!vivianne_core::is_parallel_safe(name), "{name}");
    }
}
