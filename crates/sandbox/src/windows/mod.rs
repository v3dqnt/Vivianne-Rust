//! Windows write restriction: a `WRITE_RESTRICTED`, Low integrity token whose restricting SIDs are granted on the
//! writable folders only. Port of `@deepseek-ai/dsh-sandbox-windows-acl`; see this folder's README for what it
//! changes on disk, which it does not undo.
// The token, ACL and process calls need `unsafe`; each block says why it is sound.
#![allow(unsafe_code)]

mod acl;
mod process;
mod token;

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use sha2::{Digest as _, Sha256};

use crate::policy::Mode;

/// The capability SID of one writable folder: the same folder always gets the same one, so its grant is made once.
fn write_sid(kind: &str, folder: &Path) -> String {
    let digest = Sha256::new()
        .chain_update(kind)
        .chain_update(folder.to_string_lossy().to_lowercase())
        .finalize();
    let part = |at: usize| {
        let word = u32::from_le_bytes([digest[at], digest[at + 1], digest[at + 2], digest[at + 3]]);
        word % ((1 << 30) - 1) + 1
    };
    format!("S-1-4-{}-{}", part(0), part(4))
}

/// A canonical path without the `\\?\` prefix, which the security calls and `cmd` do not take.
fn plain(path: &Path) -> Result<PathBuf, String> {
    let real = std::fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if !real.is_dir() {
        return Err(format!("{} is not a folder", path.display()));
    }
    let text = real.to_string_lossy();
    Ok(PathBuf::from(
        text.strip_prefix(r"\\?\").unwrap_or(&text).to_owned(),
    ))
}

fn contains(folder: &Path, other: &Path) -> bool {
    let lower = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    lower(other).starts_with(lower(folder))
}

/// A grant relabels a whole tree for good, so a tree that holds the user's profile, the temporary folder, Windows or
/// this program is never granted.
fn may_grant(folder: &Path) -> Result<(), String> {
    let mut protected = vec![std::env::temp_dir()];
    protected.extend(std::env::var_os("USERPROFILE").map(PathBuf::from));
    protected.extend(std::env::var_os("SystemRoot").map(PathBuf::from));
    protected.extend(std::env::current_exe().ok());
    for path in protected {
        let real = std::fs::canonicalize(&path).unwrap_or(path);
        let text = real.to_string_lossy();
        let real = Path::new(text.strip_prefix(r"\\?\").unwrap_or(&text));
        if contains(folder, real) {
            return Err(format!(
                "{} cannot be a writable folder: it holds {}",
                folder.display(),
                real.display()
            ));
        }
    }
    Ok(())
}

struct Request {
    mode: Mode,
    writable: Vec<PathBuf>,
    command: Vec<OsString>,
}

/// `--mode <mode> [--writable <folder>]... -- <command>...`
fn parse(args: Vec<OsString>) -> Result<Request, String> {
    let mut args = args.into_iter();
    let (mut mode, mut writable) = (None, Vec::new());
    loop {
        let Some(flag) = args.next() else {
            return Err("missing `--` before the command".to_owned());
        };
        if flag == "--" {
            break;
        }
        let value = args
            .next()
            .ok_or_else(|| format!("missing value after {}", flag.to_string_lossy()))?;
        match flag.to_str() {
            Some("--mode") => mode = value.to_str().and_then(Mode::parse),
            Some("--writable") => writable.push(PathBuf::from(value)),
            _ => return Err(format!("unknown argument: {}", flag.to_string_lossy())),
        }
    }
    let Some(mode @ (Mode::ReadOnly | Mode::WorkspaceWrite)) = mode else {
        return Err("--mode must be read-only or workspace-write".to_owned());
    };
    if mode == Mode::ReadOnly && !writable.is_empty() {
        return Err("read-only takes no --writable".to_owned());
    }
    let command: Vec<OsString> = args.collect();
    if command.is_empty() {
        return Err("missing command after `--`".to_owned());
    }
    Ok(Request {
        mode,
        writable,
        command,
    })
}

/// A new private temporary folder, removed when dropped. One left by a killed runner holds only a grant to a SID no
/// later token carries.
struct PrivateTemp(PathBuf);

impl PrivateTemp {
    fn new() -> Result<Self, String> {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let path = std::env::temp_dir().join(format!("viv-sbx-{}-{nanos:x}", std::process::id()));
        std::fs::create_dir(&path).map_err(|e| format!("private temp folder: {e}"))?;
        Ok(Self(path))
    }
}

impl Drop for PrivateTemp {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("{}: cleanup: {error}", crate::runner::SIGNATURE);
        }
    }
}

/// Any failure here ends without the command having run.
pub(crate) fn run(args: Vec<OsString>) -> Result<i32, String> {
    let request = parse(args)?;
    let mut sids = Vec::new();
    let mut temp = None;
    if request.mode == Mode::WorkspaceWrite {
        // Everything is checked before anything is changed.
        let folders = request
            .writable
            .iter()
            .map(|folder| plain(folder).and_then(|f| may_grant(&f).map(|()| f)))
            .collect::<Result<Vec<_>, _>>()?;
        for folder in &folders {
            let sid = token::Sid::parse(&write_sid("workspace\0", folder))?;
            acl::grant_write(folder, &sid)?;
            sids.push(sid);
        }
        let private = PrivateTemp::new()?;
        let folder = plain(&private.0)?;
        let sid = token::Sid::parse(&write_sid("temp\0", &folder))?;
        acl::grant_write(&folder, &sid)?;
        // SAFETY: the runner has started no thread of its own, and Windows keeps its environment behind a lock.
        unsafe {
            std::env::set_var("TMP", &folder);
            std::env::set_var("TEMP", &folder);
        }
        sids.push(sid);
        temp = Some(private);
    }
    let restricted = token::restricted(&sids)?;
    let code = process::run(&restricted, &request.command);
    drop(temp);
    code
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_has_one_sid_and_the_temp_one_differs() {
        let a = write_sid("workspace\0", Path::new(r"C:\Work\A"));
        assert_eq!(a, write_sid("workspace\0", Path::new(r"c:\work\a")));
        assert_ne!(a, write_sid("temp\0", Path::new(r"C:\Work\A")));
        assert_ne!(a, write_sid("workspace\0", Path::new(r"C:\Work\B")));
        assert!(a.starts_with("S-1-4-"));
    }

    #[test]
    fn a_tree_holding_the_profile_temp_or_system_is_never_granted() {
        let temp = std::env::temp_dir();
        for folder in [Path::new(r"C:\"), temp.as_path()] {
            assert!(may_grant(folder).is_err(), "{}", folder.display());
        }
        let profile = PathBuf::from(std::env::var_os("USERPROFILE").expect("profile"));
        assert!(may_grant(&profile).is_err());
        assert!(may_grant(&temp.join("viv-some-project")).is_ok());
    }

    #[test]
    fn a_malformed_request_is_refused() {
        let args = |list: &[&str]| parse(list.iter().map(OsString::from).collect()).map(|_| ());
        assert!(args(&["--mode", "read-only", "--", "cmd"]).is_ok());
        assert!(args(&["--mode", "danger-full-access", "--", "cmd"]).is_err());
        assert!(args(&["--mode", "read-only", "--writable", "x", "--", "cmd"]).is_err());
        assert!(args(&["--mode", "read-only", "--"]).is_err());
        assert!(args(&["--mode", "read-only", "cmd"]).is_err());
    }
}
