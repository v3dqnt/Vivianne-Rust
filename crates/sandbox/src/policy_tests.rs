//! Modes, writable folders and escalation.

use super::*;

fn dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("viv-policy-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    std::fs::canonicalize(&dir).expect("real")
}

fn policy(mode: Mode, workspace: &Path, additional: &[PathBuf]) -> Policy {
    Policy::new(mode, workspace, additional).expect("policy")
}

#[test]
fn only_workspace_write_opens_folders() {
    let ws = dir("ws");
    for mode in [Mode::ReadOnly, Mode::DangerFullAccess] {
        assert!(
            policy(mode, &ws, &[])
                .writable_roots()
                .expect("roots")
                .is_empty(),
            "{mode:?}"
        );
    }
    let roots = policy(Mode::WorkspaceWrite, &ws, &[])
        .writable_roots()
        .expect("roots");
    assert!(roots.contains(&ws));
    assert!(roots.len() >= 2, "temporary areas too");
    assert!(Policy::new(Mode::ReadOnly, Path::new("relative"), &[]).is_err());
}

#[test]
fn an_additional_folder_must_exist_and_be_given_by_its_real_path() {
    let (ws, extra) = (dir("ws2"), dir("extra"));
    let ok = policy(Mode::WorkspaceWrite, &ws, &[extra.clone(), extra.clone()]);
    assert_eq!(ok.additional.len(), 1);
    assert!(ok.workspace_roots().expect("roots").contains(&extra));
    let missing = policy(Mode::WorkspaceWrite, &ws, &[extra.join("nope")]);
    assert!(matches!(
        missing.workspace_roots(),
        Err(PolicyError::Root { .. })
    ));
    let file = extra.join("f");
    std::fs::write(&file, "x").expect("file");
    assert!(
        policy(Mode::WorkspaceWrite, &ws, &[file])
            .workspace_roots()
            .is_err()
    );
    // Written as text: joining onto a Windows `\\?\` path would fold the `..` away before it is looked at.
    let plain = extra
        .to_string_lossy()
        .trim_start_matches(r"\\?\")
        .to_owned();
    let name = extra
        .file_name()
        .expect("name")
        .to_string_lossy()
        .into_owned();
    let sep = std::path::MAIN_SEPARATOR;
    let indirect = PathBuf::from(format!("{plain}{sep}..{sep}{name}"));
    assert!(
        policy(Mode::WorkspaceWrite, &ws, &[indirect])
            .workspace_roots()
            .is_err(),
        "not the physical spelling"
    );
}

#[test]
fn a_session_switch_wins_over_the_default() {
    assert_eq!(standing_mode(Mode::ReadOnly, None), Mode::ReadOnly);
    assert_eq!(
        standing_mode(Mode::ReadOnly, Some(Mode::WorkspaceWrite)),
        Mode::WorkspaceWrite
    );
    assert!(
        policy(Mode::ReadOnly, &dir("d"), &[])
            .describe()
            .starts_with("Current file policy: read-only")
    );
}

#[test]
fn escalation_needs_both_arguments_and_only_goes_wider_with_approval() {
    assert_eq!(check_escalation_args(None, None), Ok(None));
    assert_eq!(
        check_escalation_args(Some("workspace-write"), None),
        Err(PolicyError::MissingJustification)
    );
    assert_eq!(
        check_escalation_args(None, Some("why")),
        Err(PolicyError::StrayJustification)
    );
    assert_eq!(
        check_escalation_args(Some("workspace-write"), Some("  ")),
        Err(PolicyError::EmptyJustification)
    );
    assert!(
        check_escalation_args(Some("read-only"), Some("w")).is_err(),
        "not a target"
    );
    assert!(check_escalation_args(Some("root"), Some("w")).is_err());

    let mut asked_with = String::new();
    let got = escalate(
        Mode::ReadOnly,
        Mode::WorkspaceWrite,
        "to save the file",
        |why| {
            why.clone_into(&mut asked_with);
            true
        },
    );
    assert_eq!(got, Ok(Some(Mode::WorkspaceWrite)));
    assert_eq!(
        asked_with,
        "escalate sandbox to workspace-write: to save the file"
    );
    assert_eq!(
        escalate(Mode::ReadOnly, Mode::DangerFullAccess, "w", |_| false),
        Ok(None),
        "refused"
    );
    let narrower = escalate(Mode::DangerFullAccess, Mode::WorkspaceWrite, "w", |_| {
        panic!("not asked")
    });
    assert!(matches!(narrower, Err(PolicyError::NotWider { .. })));
    let same = escalate(Mode::WorkspaceWrite, Mode::WorkspaceWrite, "w", |_| {
        panic!("nothing to approve")
    });
    assert_eq!(same, Ok(Some(Mode::WorkspaceWrite)));
}
