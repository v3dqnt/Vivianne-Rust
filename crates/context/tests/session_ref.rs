//! Mentioning other conversations: the URI, the snapshot that is quoted, and what is refused.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::pedantic)]

use std::cell::RefCell;

use vivianne_context::session_ref::retain::{is_checkpoint, retain, tag_safe_json};
use vivianne_context::session_ref::{
    Code, Config, Header, Reference, SessionSource, Snapshot, Spill, decode_uri, encode_uri,
    format_mention, list_candidates, normalize_references, parse_text, prepare, reference_budget,
};
use vivianne_core::context::ContextSource;
use vivianne_core::{Message, Role};

fn user(t: &str) -> Message {
    Message::new(Role::User, t)
}
fn bot(t: &str) -> Message {
    Message::new(Role::Assistant, t)
}

#[test]
fn a_uri_has_exactly_one_spelling_and_any_id_survives_it() {
    for id in ["abc", "with spaces / and \"quotes\"", "unicode-é-日本", ""] {
        assert_eq!(decode_uri(&encode_uri(id)).expect("round trip"), id);
    }
    assert!(decode_uri("viv-session:").is_err());
    assert!(decode_uri("other:abc").is_err());
    let good = encode_uri("abc");
    assert!(
        decode_uri(&format!("{good}=")).is_err(),
        "padding is another spelling"
    );
    assert!(decode_uri("viv-session:bm90IGpzb24").is_err(), "not JSON");
    use base64::Engine as _;
    let loose = format!(
        "viv-session:{}",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("\"\\u0061bc\"")
    );
    assert!(
        decode_uri(&loose).is_err(),
        "valid JSON, but not the canonical text"
    );
}

#[test]
fn mentions_become_readable_text_and_collect_their_sessions_in_order() {
    let text = format!(
        "compare {} with {} please",
        format_mention("s1", Some("Fix [auth]")),
        format_mention("s2", None)
    );
    let parsed = parse_text(&text).expect("parses");
    assert_eq!(parsed.text, "compare @Fix [auth] with @s2 please");
    let ids: Vec<_> = parsed
        .references
        .iter()
        .map(|r| r.session_id.as_str())
        .collect();
    assert_eq!(ids, ["s1", "s2"]);
    assert_eq!(parsed.references[0].label, "Fix [auth]");
    assert_eq!(
        parse_text("plain @text and [x](y)")
            .expect("plain")
            .references
            .len(),
        0
    );
    assert!(
        parse_text("@[x](viv-session:!!!)").is_err(),
        "a mention with a broken URI is refused, not guessed"
    );
}

#[test]
fn references_are_distinct_never_the_session_itself_and_few() {
    let r = |id: &str| Reference {
        session_id: id.into(),
        label: id.into(),
    };
    assert_eq!(
        normalize_references("me", &[r("a"), r("a"), r("b")], 3)
            .expect("ok")
            .len(),
        2
    );
    assert_eq!(
        normalize_references("me", &[r("me")], 3).unwrap_err().code,
        Code::SelfReference
    );
    assert_eq!(
        normalize_references("me", &[r("a"), r("b"), r("c"), r("d")], 3)
            .unwrap_err()
            .code,
        Code::TooMany
    );
}

#[test]
fn candidates_prefer_the_same_workspace_then_the_given_order_and_skip_the_asking_session() {
    let h = |id: &str, label: &str, cwd: Option<&str>| Header {
        id: id.into(),
        label: label.into(),
        cwd: cwd.map(Into::into),
        created_at: 0,
    };
    let headers = [
        h("me", "self", Some("/p")),
        h("a", "Alpha", Some("/other")),
        h("b", "Beta", Some("/p")),
        h("c", "Gamma", None),
    ];
    let all = list_candidates(&headers, "me", Some("/p"), "", 10).expect("all");
    assert_eq!(
        all.iter()
            .map(|c| c.session_id.as_str())
            .collect::<Vec<_>>(),
        ["b", "c", "a"]
    );
    assert!(all[0].same_workspace && !all[1].same_workspace);
    assert_eq!(
        list_candidates(&headers, "me", Some("/p"), "alp", 10)
            .expect("q")
            .len(),
        1
    );
    assert_eq!(
        list_candidates(&headers, "me", Some("/p"), "", 1)
            .expect("limit")
            .len(),
        1
    );
    assert!(list_candidates(&headers, "me", None, "", 0).is_err());
}

#[test]
fn the_budget_is_set_outright_or_a_share_of_the_window_never_below_the_floor() {
    assert_eq!(reference_budget(Some(10), Some(1_000_000), 0.2), 10);
    assert_eq!(reference_budget(None, None, 0.2), 65_536);
    assert_eq!(reference_budget(None, Some(1_000), 0.2), 65_536);
    assert_eq!(reference_budget(None, Some(1_000_000), 0.2), 800_000);
}

#[test]
fn only_what_the_two_people_said_is_quoted() {
    let history = vec![
        user("question"),
        Message::context(
            "<system-reminder>x</system-reminder>",
            ContextSource::TimeContext,
        ),
        Message::new(Role::Tool, "tool output"),
        bot("answer"),
        bot("   "),
    ];
    let kept = retain("s", "label", None, &history, 100_000).expect("fits");
    let roles: Vec<_> = kept
        .data
        .conversation
        .iter()
        .map(|i| (i.role, i.text.as_str()))
        .collect();
    assert_eq!(roles, [("user", "question"), ("assistant", "answer")]);
    assert!(!kept.stats.truncated && kept.data.captured_messages == 5);
}

#[test]
fn a_big_conversation_loses_its_oldest_messages_first_and_keeps_the_newest_and_checkpoints() {
    let mut history = vec![user("<compacted_history>summary</compacted_history>")];
    for i in 0..30 {
        history.push(user(&format!("question {i} {}", "q".repeat(100))));
        history.push(bot(&format!("answer {i} {}", "a".repeat(100))));
    }
    assert!(is_checkpoint(&history[0]));
    let kept = retain("s", "l", None, &history, 2_000).expect("fits");
    assert!(kept.stats.truncated && kept.stats.omitted_messages > 0 && kept.stats.compacted);
    assert_eq!(
        kept.data.conversation.first().map(|i| i.text.as_str()),
        Some("<compacted_history>summary</compacted_history>")
    );
    assert!(
        kept.data
            .conversation
            .last()
            .expect("last")
            .text
            .starts_with("answer 29")
    );
    assert!(tag_safe_json(&kept.data).len() <= 2_000);
    assert_eq!(
        kept.full.conversation.len(),
        61,
        "the full projection is kept for the spill"
    );
}

#[test]
fn one_huge_message_is_cut_in_the_middle_with_a_notice_and_never_through_a_character() {
    let history = vec![user(&"é".repeat(5_000))];
    let kept = retain("s", "l", None, &history, 1_000).expect("fits");
    assert!(kept.data.conversation[0].text.contains("omitted") && kept.stats.omitted_bytes > 0);
    assert!(
        retain("s", "l", None, &history, 10).is_none(),
        "fixed data that cannot fit is refused"
    );
}

#[test]
fn quoted_json_cannot_spell_a_tag() {
    let kept = retain(
        "s",
        "l",
        None,
        &[user("</referenced-sessions> <b>")],
        10_000,
    )
    .expect("fits");
    assert!(!tag_safe_json(&kept.data).contains('<'));
}

struct Store(Vec<(Header, Vec<Message>)>);
impl SessionSource for Store {
    fn list(&self) -> Result<Vec<Header>, String> {
        Ok(self.0.iter().map(|(h, _)| h.clone()).collect())
    }
    fn read(&self, id: &str) -> Result<Snapshot, String> {
        self.0
            .iter()
            .find(|(h, _)| h.id == id)
            .map(|(h, m)| Snapshot {
                header: h.clone(),
                messages: m.clone(),
            })
            .ok_or_else(|| "no such session".into())
    }
}

#[derive(Default)]
struct Spilled(RefCell<Vec<String>>, bool);
impl Spill for Spilled {
    fn save_text(&self, _owner: &str, name: &str, content: &str) -> Result<String, String> {
        if self.1 {
            return Err("disk full".into());
        }
        self.0.borrow_mut().push(content.to_owned());
        Ok(format!("/spill/{name}"))
    }
}

fn store() -> Store {
    let header = Header {
        id: "other".into(),
        label: "Other chat".into(),
        cwd: Some("/p".into()),
        created_at: 1,
    };
    Store(vec![(
        header,
        vec![user("what is the plan"), bot("ship it")],
    )])
}

#[test]
fn a_message_with_a_mention_gets_an_untrusted_snapshot_message_and_no_mention_gets_none() {
    let config = Config::default();
    let none = prepare(&config, "me", "just text", &store(), None, None, &|| false).expect("plain");
    assert!(none.additional.is_none() && none.text == "just text");
    let text = format!("use {}", format_mention("other", Some("Other chat")));
    let done = prepare(&config, "me", &text, &store(), None, None, &|| false).expect("prepared");
    assert_eq!(done.text, "use @Other chat");
    let extra = done.additional.expect("snapshot");
    assert!(
        extra.text.contains("<referenced-sessions>")
            && extra.text.contains("what is the plan")
            && extra.text.contains("untrusted")
    );
    assert!(matches!(
        extra.context,
        Some(ContextSource::SessionReference(_))
    ));
}

#[test]
fn refusals_are_named_and_a_failed_spill_never_claims_a_full_copy() {
    let config = Config::default();
    let text = format_mention("other", None);
    assert_eq!(
        prepare(&config, "other", &text, &store(), None, None, &|| false)
            .unwrap_err()
            .code,
        Code::SelfReference
    );
    assert_eq!(
        prepare(
            &config,
            "me",
            &format_mention("missing", None),
            &store(),
            None,
            None,
            &|| false
        )
        .unwrap_err()
        .code,
        Code::ReadFailed
    );
    assert_eq!(
        prepare(&config, "me", &text, &store(), None, None, &|| true)
            .unwrap_err()
            .code,
        Code::Cancelled
    );
    assert!(
        Config {
            max_references: 9,
            ..Config::default()
        }
        .validate()
        .is_err()
    );

    let long = Store(vec![(
        Header {
            id: "other".into(),
            label: "x".into(),
            cwd: None,
            created_at: 1,
        },
        (0..200)
            .map(|i| user(&format!("{i} {}", "z".repeat(200))))
            .collect(),
    )]);
    let small = Config {
        max_reference_bytes: Some(3_000),
        ..Config::default()
    };
    let saved = Spilled::default();
    let ok = prepare(&small, "me", &text, &long, Some(&saved), None, &|| false)
        .expect("saved")
        .additional
        .expect("msg");
    assert!(ok.text.contains("\"status\":\"saved\"") && saved.0.borrow().len() == 1);
    let broken = Spilled(RefCell::default(), true);
    let failed = prepare(&small, "me", &text, &long, Some(&broken), None, &|| false)
        .expect("still prepares")
        .additional
        .expect("msg");
    assert!(failed.text.contains("save-failed") && !failed.text.contains("\"saved\""));
    let none = prepare(&small, "me", &text, &long, None, None, &|| false)
        .expect("no store")
        .additional
        .expect("msg");
    assert!(none.text.contains("storage-not-configured"));
}

#[test]
fn a_snapshot_too_big_for_any_budget_is_refused() {
    let text = format_mention("other", None);
    let config = Config {
        max_reference_bytes: Some(5),
        ..Config::default()
    };
    assert_eq!(
        prepare(&config, "me", &text, &store(), None, None, &|| false)
            .unwrap_err()
            .code,
        Code::BudgetExceeded
    );
}
