//! `@file` completion: the token grammar, the mention it writes back, and the bounded search.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::pedantic)]

mod common;

use common::Scratch;
use vivianne_context::file_ref::{
    Candidate, Config, Kind, Stop, WorkspaceSearch, active_at_token, format_file_mention,
};

fn token(line: &str) -> Option<(String, String, bool)> {
    active_at_token(line, line.len()).map(|t| (t.prefix, t.query, t.quoted))
}

#[test]
fn an_at_starts_a_token_only_at_the_start_of_a_word() {
    assert_eq!(
        token("@src/ma"),
        Some(("@src/ma".into(), "src/ma".into(), false))
    );
    assert_eq!(
        token("look at @rea"),
        Some(("@rea".into(), "rea".into(), false))
    );
    assert_eq!(
        token("mail me@example.com"),
        None,
        "an address is not a mention"
    );
    assert_eq!(token("no mention"), None);
    assert_eq!(token("@"), Some(("@".into(), String::new(), false)));
}

#[test]
fn a_quote_keeps_spaces_inside_the_token_until_it_closes() {
    assert_eq!(
        token("see @\"my dir/fi"),
        Some(("@\"my dir/fi".into(), "my dir/fi".into(), true))
    );
    assert_eq!(token("see @\"done\" and more"), None);
    assert_eq!(
        active_at_token("hello @sr", 100),
        None,
        "a cursor off the line is nothing"
    );
}

#[test]
fn a_chosen_path_is_written_so_it_reads_back_as_the_same_path() {
    let file = |p: &str| Candidate {
        path: p.into(),
        kind: Kind::File,
    };
    let dir = |p: &str| Candidate {
        path: p.into(),
        kind: Kind::Directory,
    };
    assert_eq!(
        format_file_mention(&file("src/main.rs"), false).as_deref(),
        Some("@src/main.rs")
    );
    assert_eq!(
        format_file_mention(&dir("src"), false).as_deref(),
        Some("@src/")
    );
    assert_eq!(
        format_file_mention(&file("my notes.md"), false).as_deref(),
        Some("@\"my notes.md\"")
    );
    assert_eq!(
        format_file_mention(&dir("my dir"), false).as_deref(),
        Some("@\"my dir/"),
        "the quote stays open for the next level"
    );
    assert_eq!(format_file_mention(&file("a\"b"), false), None);
    assert_eq!(format_file_mention(&file("a\nb"), false), None);
}

fn search(s: &Scratch) -> WorkspaceSearch {
    WorkspaceSearch::new(s.path(), Config::default()).expect("search")
}

fn paths(found: Vec<Candidate>) -> Vec<String> {
    found.into_iter().map(|c| c.path).collect()
}

#[test]
fn a_name_finds_files_anywhere_best_match_first_and_skips_build_output() {
    let s = Scratch::new("search");
    s.write("src/main.rs", "");
    s.write("src/domain/mapper.rs", "");
    s.write("README.md", "");
    s.write("node_modules/main.js", "");
    s.write("target/main.o", "");
    let found = paths(search(&s).list("main", &Stop::new()).expect("list"));
    assert_eq!(found.first().map(String::as_str), Some("src/main.rs"));
    assert!(
        !found
            .iter()
            .any(|p| p.contains("node_modules") || p.starts_with("target"))
    );
}

#[test]
fn a_path_with_a_slash_lists_that_folder_directly() {
    let s = Scratch::new("dir");
    s.write("src/a.rs", "");
    s.write("src/inner/b.rs", "");
    s.write("other/c.rs", "");
    let found = search(&s).list("src/", &Stop::new()).expect("list");
    let names: Vec<_> = found.iter().map(|c| (c.path.as_str(), c.kind)).collect();
    assert!(
        names.contains(&("src/a.rs", Kind::File))
            && names.contains(&("src/inner", Kind::Directory))
    );
    assert!(!names.iter().any(|(p, _)| p.starts_with("other")));
    assert!(
        search(&s)
            .list("src/in", &Stop::new())
            .expect("list")
            .iter()
            .all(|c| c.path.starts_with("src/in"))
    );
}

#[test]
fn results_are_capped_and_the_order_is_fixed() {
    let s = Scratch::new("cap");
    for i in 0..40 {
        s.write(&format!("f/file{i:02}.txt"), "");
    }
    let engine = WorkspaceSearch::new(
        s.path(),
        Config {
            max_results: 5,
            ..Config::default()
        },
    )
    .expect("search");
    let a = paths(engine.list("file", &Stop::new()).expect("a"));
    assert_eq!(a.len(), 5);
    assert_eq!(a, paths(engine.list("file", &Stop::new()).expect("b")));
}

#[test]
fn the_index_follows_the_disk_after_an_invalidation_and_a_stop_ends_the_search() {
    let s = Scratch::new("fresh");
    s.write("one.txt", "");
    let engine = search(&s);
    assert!(paths(engine.list("two", &Stop::new()).expect("before")).is_empty());
    s.write("two.txt", "");
    engine.invalidate();
    // A stale index keeps answering while a replacement is built in the background; it catches up shortly.
    let mut caught_up = false;
    for _ in 0..200 {
        if paths(engine.list("two", &Stop::new()).expect("after")) == ["two.txt"] {
            caught_up = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(
        caught_up,
        "the replacement index never replaced the stale one"
    );
    let stop = Stop::new();
    stop.stop();
    assert!(engine.list("two", &stop).is_err());
    engine.dispose();
    assert!(
        engine
            .list("two", &Stop::new())
            .expect("disposed")
            .is_empty()
    );
}

#[test]
fn bad_settings_are_refused_at_the_start() {
    let s = Scratch::new("config");
    assert!(
        WorkspaceSearch::new(
            s.path(),
            Config {
                max_results: 0,
                ..Config::default()
            }
        )
        .is_err()
    );
    assert!(
        WorkspaceSearch::new(
            s.path(),
            Config {
                excluded_directories: vec!["a/b".into()],
                ..Config::default()
            }
        )
        .is_err()
    );
}
