//! Immutable content-addressed objects on disk. A write goes to a staging file, is flushed, then linked into its
//! final place: a crash leaves either nothing or a complete object. An object that already exists is checked by its
//! digest and kept. Objects are read-only and private to the person.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::error::{AttachmentError, Code, Error, Result, fail};
use crate::limiter::Cancel;

const CHUNK: usize = 1 << 16;

/// A fully written staging file and what was measured while writing it.
#[derive(Debug)]
pub struct Staged {
    pub path: PathBuf,
    pub sha256: String,
    pub bytes: u64,
}

fn write_failed(cause: &std::io::Error) -> Error {
    // The cause names a host path on some systems: it goes to no message.
    let _ = cause;
    AttachmentError::new(Code::AttachmentWriteFailed, "Unable to persist attachment.").into()
}

fn hex(digest: &[u8]) -> String {
    use std::fmt::Write as _;
    digest.iter().fold(String::with_capacity(64), |mut s, b| {
        let _ = write!(s, "{b:02x}");
        s
    })
}

/// The SHA-256 digest of a file, read in chunks.
pub fn digest_file(path: &Path) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; CHUNK];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            return Ok(hex(&hash.finalize()));
        }
        hash.update(&buf[..n]);
    }
}

/// The digest of what a hasher has seen so far, in hex.
#[must_use]
pub fn digest_from(hash: &Sha256) -> String {
    hex(&hash.clone().finalize())
}

/// The SHA-256 digest of bytes, in hex.
#[must_use]
pub fn digest_bytes(data: &[u8]) -> String {
    hex(&Sha256::digest(data))
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> std::io::Result<()> {
    File::open(path)?.sync_all()
}

// Windows cannot open a directory as a file; NTFS journals the entry itself.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn sync_directory(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_private_dir(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn make_private_dir(_path: &Path) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn make_read_only(path: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o400))
}

#[cfg(not(unix))]
fn make_read_only(path: &Path) -> std::io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    permissions.set_readonly(true);
    fs::set_permissions(path, permissions)
}

/// Makes a file writable again. Only tests and cleanup of a staging name need this.
pub fn make_writable(path: &Path) -> std::io::Result<()> {
    let mut permissions = fs::metadata(path)?.permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    fs::set_permissions(path, permissions)
}

/// Creates a private folder and makes every folder entry from it up to `boundary` durable. An entry that
/// "already existed" may still be unsynced in the process that made it, so every level is synced again.
pub fn ensure_durable_directory(path: &Path, boundary: &Path) -> Result<()> {
    let inner = || -> std::io::Result<()> {
        fs::create_dir_all(path)?;
        make_private_dir(path)?;
        let mut level = path.to_path_buf();
        while let Some(parent) = level.parent().map(Path::to_path_buf) {
            sync_directory(&parent)?;
            if level == boundary || parent == level || !level.starts_with(boundary) {
                break;
            }
            level = parent;
        }
        Ok(())
    };
    inner().map_err(|e| write_failed(&e))
}

fn remove_staged(path: &Path) {
    // The name may already be gone after a successful link; that is not a fault.
    if let Err(e) = fs::remove_file(path)
        && e.kind() != std::io::ErrorKind::NotFound
    {
        let _ = make_writable(path).and_then(|()| fs::remove_file(path));
    }
}

/// Streams `reader` into a new staging file under `root/tmp`, hashing as it goes. Stops early when cancelled or when
/// more than `max_bytes` arrive; the staging file is removed on every failure.
pub fn stage(
    root: &Path,
    boundary: &Path,
    reader: &mut dyn Read,
    max_bytes: Option<u64>,
    cancel: &Cancel,
) -> Result<Staged> {
    cancel.check()?;
    let staging = root.join("tmp");
    ensure_durable_directory(&staging, boundary)?;
    let path = staging.join(uuid::Uuid::new_v4().to_string());
    let outcome = (|| -> Result<(String, u64)> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|e| write_failed(&e))?;
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        let mut buf = vec![0u8; CHUNK];
        loop {
            cancel.check()?;
            let n = reader.read(&mut buf).map_err(|e| write_failed(&e))?;
            if n == 0 {
                break;
            }
            bytes += n as u64;
            if max_bytes.is_some_and(|max| bytes > max) {
                return fail(
                    Code::FileTooLarge,
                    "File is larger than the configured size limit.",
                );
            }
            hash.update(&buf[..n]);
            file.write_all(&buf[..n]).map_err(|e| write_failed(&e))?;
        }
        cancel.check()?;
        file.sync_all().map_err(|e| write_failed(&e))?;
        Ok((hex(&hash.finalize()), bytes))
    })();
    match outcome {
        Ok((sha256, bytes)) => Ok(Staged {
            path,
            sha256,
            bytes,
        }),
        Err(e) => {
            remove_staged(&path);
            Err(e)
        }
    }
}

/// Links `source` to `target`; an existing target must hold the same digest.
fn link_in_place(source: &Path, target: &Path, sha256: &str) -> Result<()> {
    match fs::hard_link(source, target) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            if digest_file(target).map_err(|e| write_failed(&e))? == sha256 {
                Ok(())
            } else {
                fail(
                    Code::AttachmentCorrupt,
                    "Stored attachment failed integrity verification.",
                )
            }
        }
        Err(e) => Err(write_failed(&e)),
    }
}

fn sync_up_to(parent: &Path, root: &Path) -> Result<()> {
    let mut level = parent.to_path_buf();
    while level != root {
        sync_directory(&level).map_err(|e| write_failed(&e))?;
        match level.parent() {
            Some(up) if up != level => level = up.to_path_buf(),
            _ => break,
        }
    }
    Ok(())
}

/// Publishes a staged object at `target`.
pub fn publish_staged(root: &Path, boundary: &Path, target: &Path, staged: &Staged) -> Result<()> {
    let result = (|| -> Result<()> {
        let parent = target
            .parent()
            .ok_or_else(|| write_failed(&std::io::Error::other("no parent")))?;
        ensure_durable_directory(parent, boundary)?;
        link_in_place(&staged.path, target, &staged.sha256)?;
        // Windows shares the read-only attribute across hard links and then refuses to remove either name, so the
        // staging name goes first.
        remove_staged(&staged.path);
        make_read_only(target).map_err(|e| write_failed(&e))?;
        sync_up_to(parent, root)
    })();
    if result.is_err() {
        remove_staged(&staged.path);
    }
    result
}

/// Publishes `data` at `target`, which must be its digest's place.
pub fn publish_object(
    root: &Path,
    boundary: &Path,
    target: &Path,
    data: &[u8],
    sha256: &str,
) -> Result<()> {
    let mut reader = data;
    let staged = stage(root, boundary, &mut reader, None, &Cancel::new())?;
    if staged.sha256 != sha256 {
        remove_staged(&staged.path);
        return fail(
            Code::AttachmentCorrupt,
            "Attachment bytes do not match their publication digest.",
        );
    }
    publish_staged(root, boundary, target, &staged)
}

/// Publishes another name for an object that is already stored.
pub fn publish_alias(
    root: &Path,
    boundary: &Path,
    source: &Path,
    target: &Path,
    sha256: &str,
) -> Result<()> {
    let parent = target
        .parent()
        .ok_or_else(|| write_failed(&std::io::Error::other("no parent")))?;
    ensure_durable_directory(parent, boundary)?;
    link_in_place(source, target, sha256)?;
    make_read_only(target).map_err(|e| write_failed(&e))?;
    sync_up_to(parent, root)
}
