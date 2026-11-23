//! The projects the user works in: a name and a folder each, kept in `~/.viv/agent/projects.json`, the file the
//! old app used. A project can carry more there than this app knows (mode, icon, banner, a multiplayer peer);
//! those fields are read into `rest` and written back as they were, so using this app never loses them.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// One project.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub created_at: String,
    /// The project's folder. Projects from before folders existed have none.
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    /// Everything else the old app kept on a project.
    #[serde(flatten)]
    pub rest: Map<String, Value>,
}

/// The whole file: other keys (the thread assignments) are kept too.
#[derive(Debug, Default, Serialize, Deserialize)]
struct Data {
    #[serde(default)]
    projects: Vec<Project>,
    #[serde(flatten)]
    rest: Map<String, Value>,
}

fn file() -> std::path::PathBuf {
    vivianne_ai::paths::viv_home_path(&["agent", "projects.json"])
}

fn read(path: &Path) -> Data {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write(path: &Path, data: &Data) -> Result<(), String> {
    vivianne_ai::paths::write_json_atomic(path, data)
}

/// Every project that has a folder, in the order they were added.
#[must_use]
pub fn list() -> Vec<Project> {
    list_in(&file())
}

/// Puts a folder in the list (once: the same folder is not added twice) and returns its project.
///
/// # Errors
/// The file cannot be written.
pub fn add_folder(folder: &Path) -> Result<Project, String> {
    add_folder_in(&file(), folder)
}

fn list_in(path: &Path) -> Vec<Project> {
    read(path)
        .projects
        .into_iter()
        .filter(|p| p.path.is_some())
        .collect()
}

fn same(a: &str, b: &Path) -> bool {
    Path::new(a) == b
}

fn add_folder_in(path: &Path, folder: &Path) -> Result<Project, String> {
    let mut data = read(path);
    if let Some(found) = data
        .projects
        .iter()
        .find(|p| p.path.as_deref().is_some_and(|p| same(p, folder)))
    {
        return Ok(found.clone());
    }
    let name = folder.file_name().map_or_else(
        || folder.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let project = Project {
        id: uuid::Uuid::new_v4().to_string(),
        name,
        created_at: crate::store::now().to_string(),
        path: Some(folder.display().to_string()),
        color: None,
        rest: Map::new(),
    };
    data.projects.push(project.clone());
    write(path, &data)?;
    Ok(project)
}

/// Who a project is paired with, for working together over a connection (the `multiplayer` field of a project, as
/// the old app wrote it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pairing {
    pub peer_id: String,
    pub peer_display_name: Option<String>,
    /// A git remote both sides use.
    pub git_remote: Option<String>,
}

impl Project {
    /// The partner this project is paired with, if it is.
    #[must_use]
    pub fn pairing(&self) -> Option<Pairing> {
        let m = self.rest.get("multiplayer")?.as_object()?;
        let text = |key: &str| {
            m.get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .filter(|t| !t.trim().is_empty())
        };
        Some(Pairing {
            peer_id: text("peer_id")?,
            peer_display_name: text("peer_display_name"),
            git_remote: text("git_remote"),
        })
    }
}

/// The project paired with the partner whose code is `peer_id`.
#[must_use]
pub fn for_peer(peer_id: &str) -> Option<Project> {
    list()
        .into_iter()
        .find(|p| p.pairing().is_some_and(|m| m.peer_id == peer_id))
}

/// The project whose folder is `folder`, when it is paired.
#[must_use]
pub fn paired_at(folder: &Path) -> Option<Project> {
    list()
        .into_iter()
        .find(|p| p.path.as_deref().is_some_and(|path| same(path, folder)) && p.pairing().is_some())
}

/// Makes a project of `folder` paired with the partner whose code is `peer_id`: the folder is made, a git
/// repository is started in it (a repository can be shared; a plain folder cannot), and a shared remote is set when
/// one is given. A project already on that folder is paired instead of added again. Git trouble is not an error:
/// the pairing still works for the shared chat and activity, it just cannot sync code.
///
/// # Errors
/// No code, or the folder or the file cannot be written.
pub fn pair(
    folder: &Path,
    peer_id: &str,
    peer_display_name: Option<&str>,
    git_remote: Option<&str>,
) -> Result<Project, String> {
    pair_in(&file(), folder, peer_id, peer_display_name, git_remote)
}

fn pair_in(
    path: &Path,
    folder: &Path,
    peer_id: &str,
    peer_display_name: Option<&str>,
    git_remote: Option<&str>,
) -> Result<Project, String> {
    let peer_id = peer_id.trim();
    if peer_id.is_empty() {
        return Err("A paired project needs your partner's pairing code.".to_owned());
    }
    std::fs::create_dir_all(folder)
        .map_err(|e| format!("Couldn't create the project folder: {e}"))?;
    let git = |args: &[&str]| {
        vivianne_tools::proc::quiet_std("git")
            .args(args)
            .current_dir(folder)
            .output()
    };
    if !folder.join(".git").exists() {
        let _ = git(&["init", "-q"]);
    }
    let remote = git_remote.map(str::trim).filter(|r| !r.is_empty());
    if let Some(remote) = remote {
        // `add` fails when origin exists and `set-url` when it does not: one of the two lands either way.
        let _ = git(&["remote", "set-url", "origin", remote]);
        let _ = git(&["remote", "add", "origin", remote]);
    }
    let mut project = add_folder_in(path, folder)?;
    let pairing = serde_json::json!({ "peer_id": peer_id, "peer_display_name": peer_display_name, "git_remote": remote });
    project
        .rest
        .insert("multiplayer".to_owned(), pairing.clone());
    let mut data = read(path);
    if let Some(stored) = data.projects.iter_mut().find(|p| p.id == project.id) {
        stored.rest.insert("multiplayer".to_owned(), pairing);
    }
    write(path, &data)?;
    Ok(project)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("viv-projects-{}.json", uuid::Uuid::new_v4()))
    }

    #[test]
    fn a_folder_is_added_once_and_named_after_itself() {
        let file = scratch();
        let first = add_folder_in(&file, Path::new("/work/alpha")).expect("adds");
        let again = add_folder_in(&file, Path::new("/work/alpha")).expect("finds");
        assert_eq!((first.name.as_str(), first.id == again.id), ("alpha", true));
        add_folder_in(&file, Path::new("/work/beta")).expect("adds");
        assert_eq!(
            list_in(&file)
                .iter()
                .map(|p| p.name.as_str())
                .collect::<Vec<_>>(),
            ["alpha", "beta"]
        );
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn what_the_old_app_kept_survives_our_writes() {
        let file = scratch();
        let old = r#"{"projects":[{"id":"p1","name":"Old","created_at":"1","path":"/old","mode":"threads","icon":"data:x"}],"assignments":{"s1":"p1"}}"#;
        std::fs::write(&file, old).expect("writes");
        add_folder_in(&file, Path::new("/new")).expect("adds");
        let text = std::fs::read_to_string(&file).expect("reads");
        let back: Value = serde_json::from_str(&text).expect("json");
        assert_eq!(back["projects"][0]["mode"], "threads");
        assert_eq!(back["projects"][0]["icon"], "data:x");
        assert_eq!(back["assignments"]["s1"], "p1");
        assert_eq!(back["projects"].as_array().map(Vec::len), Some(2));
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_missing_or_broken_file_is_an_empty_list() {
        assert!(list_in(Path::new("/no/such/projects.json")).is_empty());
        let file = scratch();
        std::fs::write(&file, "not json").expect("writes");
        assert!(list_in(&file).is_empty());
        let _ = std::fs::remove_file(file);
    }

    #[test]
    fn a_project_is_paired_by_folder_and_found_by_its_partner() {
        let file = scratch();
        let folder = std::env::temp_dir().join(format!("viv-paired-{}", uuid::Uuid::new_v4()));
        let made = pair_in(&file, &folder, " peer123 ", Some("Bo"), Some("")).expect("pairs");
        assert_eq!(
            made.pairing(),
            Some(Pairing {
                peer_id: "peer123".into(),
                peer_display_name: Some("Bo".into()),
                git_remote: None
            })
        );
        // The same folder is paired, not added twice.
        let again = pair_in(&file, &folder, "peer456", None, None).expect("pairs again");
        assert_eq!(again.id, made.id);
        assert_eq!(list_in(&file).len(), 1);
        assert_eq!(
            list_in(&file)[0].pairing().map(|p| p.peer_id),
            Some("peer456".to_owned())
        );
        assert!(pair_in(&file, &folder, "  ", None, None).is_err());
        let _ = std::fs::remove_file(file);
        let _ = std::fs::remove_dir_all(folder);
    }
}
