//! The full-text index over a folder of saved threads.

use super::super::filters::{EventFilter, Kind};
use super::super::tests::{dir, thread};
use super::*;
use crate::store::save_to;

fn setup() -> (std::path::PathBuf, Query, Index) {
    let dir = dir("search");
    let a = thread(
        "a",
        "/p1",
        10,
        &["the parser panics on empty input", "fixed the parser"],
    );
    let b = thread(
        "b",
        "/p2",
        20,
        &["write a lexer", "the lexer is done, parser later"],
    );
    save_to(&dir, &a).expect("saved");
    save_to(&dir, &b).expect("saved");
    let q = Query::new(&dir);
    let mut index = Index::in_memory().expect("fts5 is in the bundled SQLite");
    assert_eq!(index.reconcile(&q).expect("indexed").indexed, 2);
    (dir, q, index)
}

fn words(query: &str) -> Search {
    Search {
        query: query.to_owned(),
        ..Search::default()
    }
}

#[test]
fn a_search_finds_words_ranks_them_and_names_the_best_match_of_each_thread() {
    let (dir, _q, index) = setup();
    let hits = index.search_threads(&words("parser")).expect("searched");
    assert_eq!(hits.items.len(), 2);
    assert!(hits.next_cursor.is_none());
    assert!(
        hits.items
            .iter()
            .all(|h| h.best.snippet.contains("[parser]")),
        "{:?}",
        hits.items
    );
    let only_lexer = index.search_threads(&words("lexer")).expect("searched");
    let found: Vec<&str> = only_lexer.items.iter().map(|h| h.thread.as_str()).collect();
    assert_eq!(found, ["b"]);
    // Words, not pieces of words.
    assert!(
        index
            .search_threads(&words("pars"))
            .expect("searched")
            .items
            .is_empty()
    );
    let phrase = index
        .search_threads(&words("\"fixed the parser\""))
        .expect("searched");
    assert_eq!(phrase.items.len(), 1);
    assert_eq!(phrase.items[0].thread, "a");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn what_a_person_types_is_never_an_operator() {
    let (dir, _q, index) = setup();
    let odd = [
        "parser OR lexer",
        "NEAR(parser lexer)",
        "pars*",
        "-parser",
        "parser\" AND",
        "col:umn",
        "( ) \" ' x",
    ];
    for query in odd {
        // Each either searches for the words in it or says it has none; none is a syntax error from SQLite.
        match index.search_threads(&words(query)) {
            Ok(_) | Err(QueryError::InvalidFilter(_)) => {}
            Err(other) => panic!("{query}: {other}"),
        }
    }
    assert!(matches!(
        index.search_threads(&words("  ")),
        Err(QueryError::InvalidFilter(_))
    ));
    assert!(matches!(
        index.search_threads(&words("\"\" - *")),
        Err(QueryError::InvalidFilter(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn pages_walk_every_hit_once_by_cursor_and_a_made_up_cursor_is_refused() {
    let dir = dir("pages");
    let texts: Vec<String> = (0..7).map(|i| format!("note {i} about parser")).collect();
    let refs: Vec<&str> = texts.iter().map(String::as_str).collect();
    save_to(&dir, &thread("t", "/w", 1, &refs)).expect("saved");
    let q = Query::new(&dir);
    let mut index = Index::in_memory().expect("opens");
    index.reconcile(&q).expect("indexed");
    let mut seen = Vec::new();
    let mut cursor = None;
    for _ in 0..10 {
        let ask = Search {
            query: "parser".into(),
            limit: Some(3),
            cursor: cursor.clone(),
            ..Search::default()
        };
        let page = index.search_events("t", &ask).expect("page");
        seen.extend(page.items.iter().map(|h| h.record.seq));
        cursor = page.next_cursor;
        if cursor.is_none() {
            break;
        }
    }
    seen.sort_unstable();
    assert_eq!(seen, (0..7).collect::<Vec<_>>(), "every hit once");
    for bad in ["", "x", "zz.1", "7ff8000000000000.1", "00.notanumber"] {
        let ask = Search {
            query: "parser".into(),
            cursor: Some(bad.into()),
            ..Search::default()
        };
        assert!(
            matches!(
                index.search_events("t", &ask),
                Err(QueryError::InvalidCursor)
            ),
            "{bad}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn filters_narrow_a_search_and_a_text_filter_is_refused_in_one() {
    let (dir, _q, index) = setup();
    let in_p2 = Search {
        query: "parser".into(),
        threads: vec![ThreadFilter::Workspace(vec!["/p2".into()])],
        ..Search::default()
    };
    assert_eq!(
        index.search_threads(&in_p2).expect("searched").items.len(),
        1
    );
    let assistant = Search {
        query: "parser".into(),
        events: vec![EventFilter::Kind(vec![Kind::Assistant])],
        ..Search::default()
    };
    let got = index.search_threads(&assistant).expect("searched");
    assert!(
        got.items
            .iter()
            .all(|h| h.best.record.kind == Kind::Assistant)
    );
    let none = Search {
        query: "parser".into(),
        threads: vec![ThreadFilter::Id(vec![])],
        ..Search::default()
    };
    assert!(
        index
            .search_threads(&none)
            .expect("searched")
            .items
            .is_empty()
    );
    let text = Search {
        query: "parser".into(),
        events: vec![EventFilter::Text("x".into())],
        ..Search::default()
    };
    assert!(matches!(
        index.search_threads(&text),
        Err(QueryError::InvalidFilter(_))
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_index_follows_the_threads_changed_ones_reindexed_and_gone_ones_dropped() {
    let (dir, q, mut index) = setup();
    let same = Reconciled {
        indexed: 0,
        unchanged: 2,
        removed: 0,
    };
    assert_eq!(index.reconcile(&q).expect("again"), same);
    let grown = thread(
        "a",
        "/p1",
        11,
        &[
            "the parser panics on empty input",
            "fixed the parser",
            "and the borrow checker",
        ],
    );
    save_to(&dir, &grown).expect("grown");
    assert_eq!(index.reconcile(&q).expect("changed").indexed, 1);
    assert_eq!(
        index
            .search_threads(&words("borrow"))
            .expect("searched")
            .items
            .len(),
        1
    );
    crate::store::delete_to(&dir, "b").expect("deleted");
    assert_eq!(index.reconcile(&q).expect("gone").removed, 1);
    assert!(
        index
            .search_threads(&words("lexer"))
            .expect("searched")
            .items
            .is_empty()
    );
    // A thread condensed into a shorter state still finds what was said before: those events are shadowed.
    let condensed = thread(
        "a",
        "/p1",
        12,
        &["<compacted_history>x</compacted_history>"],
    );
    save_to(&dir, &condensed).expect("condensed");
    index.reconcile(&q).expect("condensed");
    let shadow = index.search_threads(&words("borrow")).expect("searched");
    assert_eq!(shadow.items[0].best.record.surface, Surface::Shadowed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_or_foreign_index_file_is_made_again_not_trusted() {
    let dir = dir("rebuild");
    save_to(&dir, &thread("a", "/p", 1, &["hello parser"])).expect("saved");
    let path = dir.join("search.db");
    std::fs::write(&path, "this is not a database").expect("junk");
    let mut index = Index::open(&path).expect("rebuilt");
    assert_eq!(
        index.reconcile(&Query::new(&dir)).expect("indexed").indexed,
        1
    );
    drop(index);
    let raw = rusqlite::Connection::open(&path).expect("raw");
    raw.pragma_update(None, "user_version", 99).expect("set");
    drop(raw);
    let mut again = Index::open(&path).expect("rebuilt from another version");
    assert_eq!(
        again.reconcile(&Query::new(&dir)).expect("indexed").indexed,
        1
    );
    let _ = std::fs::remove_dir_all(&dir);
}
