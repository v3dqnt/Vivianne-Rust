//! The recorder against real git repositories and plain folders.

use std::path::{Path, PathBuf};

use super::*;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = vivianne_tools::proc::quiet_std("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn temp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "viv-changes-test-{name}-{}",
        uuid::Uuid::new_v4().simple()
    ));
    std::fs::create_dir_all(&dir).expect("dir");
    super::paths::canonical_path(&dir)
}

/// A repository with a committed `a.txt` and an ignore rule for `*.log`.
fn repo(name: &str) -> PathBuf {
    let dir = temp(name);
    git(&dir, &["init", "-q", "-b", "main"]);
    git(&dir, &["config", "user.email", "t@t"]);
    git(&dir, &["config", "user.name", "t"]);
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\n").expect("a");
    std::fs::write(dir.join(".gitignore"), "*.log\n").expect("ignore");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "init"]);
    dir
}

fn recorder(dir: &Path) -> Recorder {
    Recorder::new(dir, Limits::default()).expect("recorder")
}

fn names(summary: &ChangesSummary) -> Vec<&str> {
    summary.files.iter().map(|f| f.display.as_str()).collect()
}

#[tokio::test]
async fn a_run_lists_modified_new_and_deleted_files_with_line_counts() {
    let dir = repo("counts");
    let rec = recorder(&dir);
    rec.start(1).await;
    std::fs::write(dir.join("a.txt"), "one\nTWO\nthree\nfour\n").expect("edit");
    std::fs::write(dir.join("new.txt"), "x\ny\n").expect("new");
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(names(&summary), ["a.txt", "new.txt"]);
    assert_eq!((summary.files[0].added, summary.files[0].deleted), (2, 1));
    assert_eq!((summary.files[1].added, summary.files[1].deleted), (2, 0));
    assert_eq!((summary.total, summary.added, summary.deleted), (2, 4, 1));
    assert!(summary.snapshot.is_some());

    rec.start(2).await;
    std::fs::remove_file(dir.join("new.txt")).expect("remove");
    let second = rec.finish().await.expect("a summary");
    assert_eq!(names(&second), ["new.txt"]);
    assert_eq!((second.files[0].added, second.files[0].deleted), (0, 2));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_run_that_changed_nothing_says_nothing() {
    let dir = repo("quiet");
    let rec = recorder(&dir);
    rec.start(1).await;
    assert!(rec.finish().await.is_none());
    assert!(rec.latest().await.is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn what_the_user_changed_before_the_run_is_not_the_runs_change() {
    let dir = repo("earlier");
    std::fs::write(dir.join("a.txt"), "mine\n").expect("user edit");
    let rec = recorder(&dir);
    rec.start(1).await;
    std::fs::write(dir.join("b.txt"), "b\n").expect("run edit");
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(names(&summary), ["b.txt"]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn the_repository_is_left_exactly_as_it_was() {
    let dir = repo("untouched");
    let (index, head, refs) = (
        std::fs::read(dir.join(".git/index")).expect("index"),
        git(&dir, &["rev-parse", "HEAD"]),
        git(&dir, &["for-each-ref"]),
    );
    let rec = recorder(&dir);
    rec.start(1).await;
    std::fs::write(dir.join("a.txt"), "changed\n").expect("edit");
    std::fs::write(dir.join("untracked.txt"), "u\n").expect("untracked");
    rec.finish().await.expect("a summary");
    assert_eq!(std::fs::read(dir.join(".git/index")).expect("index"), index);
    assert_eq!(
        (
            git(&dir, &["rev-parse", "HEAD"]),
            git(&dir, &["for-each-ref"])
        ),
        (head, refs)
    );
    assert_eq!(
        git(&dir, &["status", "--porcelain"]),
        "M a.txt\n?? untracked.txt".replace('\n', "\n")
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_renamed_file_is_one_entry_and_compares_against_its_old_content() {
    let dir = repo("rename");
    let rec = recorder(&dir);
    rec.start(1).await;
    std::fs::rename(dir.join("a.txt"), dir.join("b.txt")).expect("rename");
    std::fs::write(dir.join("b.txt"), "one\ntwo\nthree\nextra\n").expect("edit");
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(names(&summary), ["b.txt"]);
    assert_eq!((summary.files[0].added, summary.files[0].deleted), (1, 0));
    let FileDiff::Text {
        before,
        after,
        hunks,
        ..
    } = rec.diff(1, 0).await.expect("reads").expect("a diff")
    else {
        panic!("text")
    };
    assert!(before && after && hunks.iter().flat_map(|h| &h.lines).any(|l| l == "+extra"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn an_ignored_file_an_edit_tool_changed_is_listed_from_its_copies() {
    let dir = repo("ignored");
    std::fs::write(dir.join("debug.log"), "a\nb\n").expect("log");
    let rec = recorder(&dir);
    rec.start(1).await;
    rec.capture(Path::new("debug.log")).await;
    rec.capture(Path::new("debug.log")).await; // a second edit of the path does not capture again
    std::fs::write(dir.join("debug.log"), "a\nB\nc\n").expect("edit");
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(names(&summary), ["debug.log"]);
    assert_eq!((summary.files[0].added, summary.files[0].deleted), (2, 1));
    assert!(matches!(
        rec.diff(1, 0).await.expect("reads"),
        Some(FileDiff::Text { .. })
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_tracked_file_is_counted_once_even_when_a_tool_captured_it() {
    let dir = repo("once");
    let rec = recorder(&dir);
    rec.start(1).await;
    rec.capture(Path::new("a.txt")).await;
    std::fs::write(dir.join("a.txt"), "one\ntwo\nthree\nfour\n").expect("edit");
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(summary.total, 1);
    assert_eq!(summary.files[0].added, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_folder_with_no_repository_lists_only_what_the_edit_tools_changed() {
    let dir = temp("plain");
    std::fs::write(dir.join("keep.txt"), "same\n").expect("keep");
    std::fs::write(dir.join("edit.txt"), "old\n").expect("edit");
    let rec = recorder(&dir);
    rec.start(1).await;
    rec.capture(Path::new("edit.txt")).await;
    rec.capture(Path::new("keep.txt")).await;
    rec.capture(Path::new("made.txt")).await;
    std::fs::write(dir.join("edit.txt"), "new\nmore\n").expect("edit");
    std::fs::write(dir.join("made.txt"), "hello\n").expect("made");
    std::fs::write(dir.join("shell-only.txt"), "not seen\n").expect("shell");
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(
        names(&summary),
        ["edit.txt", "made.txt"],
        "an unchanged capture and a shell-only edit are not listed"
    );
    assert!(summary.snapshot.is_none());
    assert_eq!((summary.files[0].added, summary.files[0].deleted), (2, 1));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn scratch_files_under_a_temporary_root_stay_out_of_the_summary() {
    let dir = temp("workdir-only");
    let scratch =
        std::env::temp_dir().join(format!("viv-scratch-{}.txt", uuid::Uuid::new_v4().simple()));
    std::fs::write(&scratch, "a\n").expect("scratch");
    // A working folder that is itself inside the temporary folder is still a workspace.
    let rec = recorder(&dir);
    rec.start(1).await;
    rec.capture(&scratch).await;
    std::fs::write(&scratch, "b\n").expect("scratch edit");
    assert!(rec.finish().await.is_none());
    let _ = std::fs::remove_file(&scratch);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn binary_and_oversized_files_are_listed_without_counts_or_lines() {
    let dir = temp("kinds");
    std::fs::write(dir.join("img.bin"), [0u8, 1, 2]).expect("bin");
    std::fs::write(dir.join("big.txt"), "x\n").expect("big");
    let rec = Recorder::new(
        &dir,
        Limits {
            max_file_bytes: 16,
            ..Limits::default()
        },
    )
    .expect("recorder");
    rec.start(1).await;
    rec.capture(Path::new("img.bin")).await;
    rec.capture(Path::new("big.txt")).await;
    std::fs::write(dir.join("img.bin"), [0u8, 9, 9, 9]).expect("bin2");
    std::fs::write(dir.join("big.txt"), "y".repeat(100)).expect("big2");
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(names(&summary), ["big.txt", "img.bin"]);
    let (big, bin) = (&summary.files[0], &summary.files[1]);
    assert!(big.oversized && !big.binary && (big.added, big.deleted) == (0, 0));
    assert!(bin.binary && !bin.oversized);
    assert!(matches!(
        rec.diff(1, 0).await.expect("reads"),
        Some(FileDiff::Oversized { .. })
    ));
    assert!(matches!(
        rec.diff(1, 1).await.expect("reads"),
        Some(FileDiff::Binary { .. })
    ));
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_summary_carries_at_most_the_file_cap_but_counts_them_all() {
    let dir = repo("cap");
    let rec = Recorder::new(
        &dir,
        Limits {
            max_files: 2,
            ..Limits::default()
        },
    )
    .expect("recorder");
    rec.start(1).await;
    for name in ["c.txt", "d.txt", "e.txt"] {
        std::fs::write(dir.join(name), "1\n2\n").expect("file");
    }
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(
        (summary.files.len(), summary.total, summary.added),
        (2, 3, 6)
    );
    assert!(
        rec.diff(1, 2).await.expect("reads").is_none(),
        "an index past the cap is unknown"
    );
    assert!(
        rec.diff(9, 0).await.expect("reads").is_none(),
        "so is a turn never recorded"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_folder_inside_a_repository_shows_files_above_it_as_parent_paths() {
    let dir = repo("nested");
    std::fs::create_dir_all(dir.join("app")).expect("app");
    std::fs::write(dir.join("app/in.txt"), "i\n").expect("in");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "app"]);
    let rec = recorder(&dir.join("app"));
    rec.start(1).await;
    std::fs::write(dir.join("app/in.txt"), "j\n").expect("edit in");
    std::fs::write(dir.join("a.txt"), "outside the folder\n").expect("edit above");
    let summary = rec.finish().await.expect("a summary");
    assert_eq!(names(&summary), ["../a.txt", "in.txt"]);
    assert_eq!(summary.files[0].path, dir.join("a.txt").to_string_lossy());
    assert_eq!(summary.files[1].path, "in.txt");
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn a_second_record_in_one_turn_replaces_the_first_even_when_empty() {
    let dir = temp("supersede");
    std::fs::write(dir.join("f.txt"), "a\n").expect("f");
    let rec = recorder(&dir);
    rec.start(1).await;
    rec.capture(Path::new("f.txt")).await;
    std::fs::write(dir.join("f.txt"), "b\n").expect("edit");
    assert!(rec.finish().await.is_some());
    // The same turn again, with the file put back the way it was.
    rec.start(1).await;
    rec.capture(Path::new("f.txt")).await;
    let again = rec
        .finish()
        .await
        .expect("the empty record replaces the earlier one");
    assert_eq!(again.total, 0);
    assert_eq!(rec.summary(1).await.expect("kept").total, 0);
    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn dropping_the_recorder_removes_its_temporary_folder() {
    let dir = temp("drop");
    std::fs::write(dir.join("f.txt"), "a\n").expect("f");
    let rec = recorder(&dir);
    rec.start(1).await;
    rec.capture(Path::new("f.txt")).await;
    let mine = rec.scratch_dir().await.expect("scratch");
    assert!(mine.exists());
    drop(rec);
    assert!(!mine.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_file_writing_tools_name_paths_to_capture() {
    use serde_json::json;
    assert_eq!(
        edited_paths("write", &json!({"path": "a.rs", "content": "x"})),
        ["a.rs"]
    );
    assert_eq!(
        edited_paths("edit", &json!({"file_path": "b.rs"})),
        ["b.rs"]
    );
    assert!(edited_paths("read", &json!({"path": "a.rs"})).is_empty());
    assert!(edited_paths("shell", &json!({"command": "rm a.rs"})).is_empty());
    assert!(edited_paths("write", &json!({"path": ""})).is_empty());
}

#[test]
fn limits_must_be_positive() {
    assert!(Limits::default().validate().is_ok());
    assert!(
        Limits {
            max_files: 0,
            ..Limits::default()
        }
        .validate()
        .is_err()
    );
    assert!(
        Recorder::new(
            Path::new("."),
            Limits {
                diff_timeout_ms: 0,
                ..Limits::default()
            }
        )
        .is_err()
    );
}
