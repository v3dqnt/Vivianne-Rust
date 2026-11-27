//! Tests for `log.rs`.

use vivianne_core::Role;

use super::*;

const ID: &str = "55555555-5555-5555-5555-555555555555";

fn dir(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("viv-log-{name}-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).expect("dir");
    dir
}

fn thread(texts: &[&str]) -> SavedThread {
    SavedThread {
        id: ID.into(),
        workspace: "/w".into(),
        title: "t".into(),
        title_source: "auto".into(),
        provider: "p".into(),
        model: "m".into(),
        mode: Mode::Ask,
        updated_at: 1,
        history: texts
            .iter()
            .enumerate()
            .map(|(i, t)| {
                Message::new(
                    if i % 2 == 0 {
                        Role::User
                    } else {
                        Role::Assistant
                    },
                    *t,
                )
            })
            .collect(),
        parent: None,
    }
}

fn text(dir: &Path) -> String {
    std::fs::read_to_string(path_of(dir, ID)).expect("log")
}

#[test]
fn a_save_that_only_adds_appends_and_leaves_what_was_written_as_it_was() {
    let dir = dir("append");
    write(&dir, &thread(&["a", "b"])).expect("first");
    let before = text(&dir);
    assert!(
        before
            .lines()
            .next()
            .is_some_and(|l| l.contains("\"type\":\"header\""))
            && before.lines().count() == 3,
        "{before}"
    );
    let mut grown = thread(&["a", "b", "c", "d"]);
    grown.updated_at = 9;
    write(&dir, &grown).expect("grown");
    let after = text(&dir);
    assert!(
        after.starts_with(&before),
        "the earlier bytes are untouched"
    );
    assert_eq!(
        after
            .lines()
            .filter(|l| l.contains("\"type\":\"message\""))
            .count(),
        4
    );
    let got = read(&dir, ID).expect("reads").expect("exists");
    assert_eq!(
        (got.thread, got.generation, got.torn_tail),
        (grown, 1, false)
    );
    write(&dir, &thread(&["a", "b", "c", "d"])).expect("nothing new");
    assert_eq!(text(&dir), after, "a save with nothing new writes nothing");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_settings_change_is_a_later_line_and_the_last_one_wins() {
    let dir = dir("meta");
    write(&dir, &thread(&["a"])).expect("first");
    let mut renamed = thread(&["a"]);
    renamed.title = "new name".into();
    renamed.title_source = "user".into();
    write(&dir, &renamed).expect("renamed");
    assert_eq!(
        text(&dir)
            .lines()
            .filter(|l| l.contains("\"type\":\"meta\""))
            .count(),
        1
    );
    assert_eq!(read(&dir, ID).unwrap().unwrap().thread.title, "new name");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_condensed_conversation_keeps_the_old_one_as_a_generation_and_starts_the_next() {
    let dir = dir("generation");
    write(&dir, &thread(&["a", "b", "c", "d"])).expect("first");
    write(&dir, &thread(&["summary"])).expect("condensed");
    assert_eq!(generations(&dir, ID), [1]);
    assert_eq!(
        read_generation(&dir, ID, 1).unwrap().unwrap().history.len(),
        4,
        "the old one is whole"
    );
    assert!(read_generation(&dir, ID, 7).unwrap().is_none());
    let now = read(&dir, ID).unwrap().unwrap();
    assert_eq!((now.thread.history.len(), now.generation), (1, 2));
    write(&dir, &thread(&["summary", "again"])).expect("grows");
    assert_eq!(generations(&dir, ID), [1], "growing starts nothing new");
    write(&dir, &thread(&[])).expect("emptied");
    assert_eq!(generations(&dir, ID), [1, 2]);
    remove(&dir, ID).expect("removed");
    assert!(generations(&dir, ID).is_empty() && !path_of(&dir, ID).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_edit_to_earlier_messages_that_does_not_shorten_rewrites_in_place() {
    let dir = dir("edit");
    write(&dir, &thread(&["a", "b", "c"])).expect("first");
    write(&dir, &thread(&["a", "[masked]", "c"])).expect("masked");
    assert!(generations(&dir, ID).is_empty());
    let got = read(&dir, ID).unwrap().unwrap();
    assert_eq!(
        (got.thread.history[1].text.as_str(), got.generation),
        ("[masked]", 1)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn half_a_line_at_the_end_reads_as_what_came_before_and_the_next_save_makes_it_whole() {
    let dir = dir("torn");
    write(&dir, &thread(&["a", "b"])).expect("first");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path_of(&dir, ID))
        .expect("open");
    file.write_all(b"{\"type\":\"message\",\"mess")
        .expect("torn");
    drop(file);
    let got = read(&dir, ID).unwrap().unwrap();
    assert_eq!((got.thread.history.len(), got.torn_tail), (2, true));
    write(&dir, &thread(&["a", "b", "c"])).expect("saved");
    let healed = read(&dir, ID).unwrap().unwrap();
    assert_eq!((healed.thread.history.len(), healed.torn_tail), (3, false));
    assert!(text(&dir).ends_with('\n') && !text(&dir).contains("\"mess\n"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_damaged_line_in_the_middle_is_an_error_and_the_file_is_set_aside_not_overwritten() {
    let dir = dir("damaged");
    write(&dir, &thread(&["a", "b"])).expect("first");
    let damaged = text(&dir).replacen("\"type\":\"message\"", "{not json", 1);
    std::fs::write(path_of(&dir, ID), &damaged).expect("damage");
    forget(&path_of(&dir, ID));
    assert!(read(&dir, ID).unwrap_err().contains("line 2 is damaged"));
    write(&dir, &thread(&["x"])).expect("a fresh one is written");
    assert_eq!(
        std::fs::read_to_string(dir.join(format!("{ID}.jsonl.damaged"))).expect("kept"),
        damaged
    );
    assert_eq!(read(&dir, ID).unwrap().unwrap().thread.history.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_log_from_a_newer_build_is_neither_read_nor_written_over() {
    let dir = dir("newer");
    let newer = format!("{{\"type\":\"header\",\"version\":99,\"id\":\"{ID}\"}}\n");
    std::fs::write(path_of(&dir, ID), &newer).expect("newer");
    assert!(read(&dir, ID).unwrap_err().contains("newer Vivianne"));
    assert!(
        write(&dir, &thread(&["a"]))
            .unwrap_err()
            .contains("newer Vivianne")
    );
    assert_eq!(text(&dir), newer, "left exactly as it was");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_line_kind_this_build_does_not_know_is_skipped_so_the_messages_stay_readable() {
    let dir = dir("unknown");
    write(&dir, &thread(&["a"])).expect("first");
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(path_of(&dir, ID))
        .expect("open");
    file.write_all(b"{\"type\":\"from_the_future\",\"x\":1}\n")
        .expect("extra");
    drop(file);
    assert_eq!(read(&dir, ID).unwrap().unwrap().thread.history.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}
