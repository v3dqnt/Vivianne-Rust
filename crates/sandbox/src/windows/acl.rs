//! The grant on one writable folder: an inherited allow for its capability SID, a deny of deleting through the parent
//! for Everyone, and a Low integrity label, applied in one call. Made once: an exact grant already there is left.
//!
//! Port of `acl.ts` (`grantWrite`). There is no revoke: a workspace's grant stands, and the private temp folder is
//! deleted whole.

use std::path::Path;
use std::ptr::null_mut;

use windows_sys::Win32::Foundation::LocalFree;
use windows_sys::Win32::Security::Authorization::{
    DENY_ACCESS, GRANT_ACCESS, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetEntriesInAclW,
    SetNamedSecurityInfoW,
};
use windows_sys::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_REVISION, AddMandatoryAce, CONTAINER_INHERIT_ACE,
    DACL_SECURITY_INFORMATION, EqualSid, GetAce, InitializeAcl, LABEL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR, PSID, SUB_CONTAINERS_AND_OBJECTS_INHERIT,
};
use windows_sys::Win32::Storage::FileSystem::{
    DELETE, FILE_DELETE_CHILD, FILE_GENERIC_WRITE, STANDARD_RIGHTS_WRITE,
};
use windows_sys::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, ACCESS_DENIED_ACE_TYPE, SYSTEM_MANDATORY_LABEL_ACE_TYPE,
    SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
};

use super::token::{Sid, entry, last_error, len32, wide};

/// What the capability SID may do: write and delete, not change permissions.
const GRANT_MASK: u32 = (FILE_GENERIC_WRITE | DELETE | FILE_DELETE_CHILD) & !STANDARD_RIGHTS_WRITE;

/// Whether `acl` holds exactly this entry.
///
/// # Safety
/// `acl` must be null or a valid ACL.
unsafe fn has_entry(acl: *const ACL, kind: u32, inheritance: u32, mask: u32, sid: &Sid) -> bool {
    if acl.is_null() {
        return false;
    }
    // SAFETY: a valid ACL, per the caller.
    let count = unsafe { (*acl).AceCount };
    (0..u32::from(count)).any(|index| {
        let mut ace = null_mut();
        // SAFETY: `index` is below the ACL's count.
        if unsafe { GetAce(acl, index, &raw mut ace) } == 0 {
            return false;
        }
        // SAFETY: every ACE starts with a header.
        let header = unsafe { *ace.cast::<ACE_HEADER>() };
        if u32::from(header.AceType) != kind || u32::from(header.AceFlags) != inheritance {
            return false;
        }
        // SAFETY: allow, deny and label entries share this layout: header, mask, then the SID.
        unsafe {
            let ace = ace.cast::<ACCESS_ALLOWED_ACE>();
            let theirs: PSID = (&raw mut (*ace).SidStart).cast();
            (*ace).Mask == mask && EqualSid(theirs, sid.as_ptr()) != 0
        }
    })
}

fn low_label(low: &Sid) -> Result<Vec<u32>, String> {
    // Header, one mandatory entry and its SID fit comfortably; the buffer is DWORD aligned as an ACL must be.
    let mut buffer = vec![0_u32; 32];
    let acl = buffer.as_mut_ptr().cast::<ACL>();
    // SAFETY: `acl` points to the buffer, whose length is passed.
    if unsafe { InitializeAcl(acl, len32(buffer.len() * 4), ACL_REVISION) } == 0 {
        return Err(last_error("InitializeAcl"));
    }
    // SAFETY: an initialised ACL with room, and a valid SID.
    let added = unsafe {
        AddMandatoryAce(
            acl,
            ACL_REVISION,
            SUB_CONTAINERS_AND_OBJECTS_INHERIT,
            SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
            low.as_ptr(),
        )
    };
    if added == 0 {
        return Err(last_error("AddMandatoryAce"));
    }
    Ok(buffer)
}

/// One writer at a time per folder, across processes.
fn locked(folder: &Path) -> Result<std::fs::File, String> {
    use sha2::{Digest as _, Sha256};
    use std::fmt::Write as _;
    let digest = Sha256::digest(folder.to_string_lossy().to_lowercase());
    let name = digest.iter().take(8).fold(String::new(), |mut name, byte| {
        let _ = write!(name, "{byte:02x}");
        name
    });
    let dir = std::env::temp_dir().join("viv-acl-locks");
    let open = || -> std::io::Result<std::fs::File> {
        std::fs::create_dir_all(&dir)?;
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(format!("{name}.lock")))?;
        file.lock()?;
        Ok(file)
    };
    open().map_err(|e| format!("grant lock: {e}"))
}

pub(super) fn grant_write(folder: &Path, sid: &Sid) -> Result<(), String> {
    let _lock = locked(folder)?;
    let (world, low) = (Sid::world()?, Sid::low()?);
    let mut name = wide(folder.as_os_str());
    let (mut dacl, mut sacl): (*mut ACL, *mut ACL) = (null_mut(), null_mut());
    let mut descriptor: PSECURITY_DESCRIPTOR = null_mut();
    // SAFETY: a NUL terminated name; the out pointers receive parts of `descriptor`, freed below.
    let code = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
            null_mut(),
            null_mut(),
            &raw mut dacl,
            &raw mut sacl,
            &raw mut descriptor,
        )
    };
    if code != 0 {
        return Err(format!(
            "reading the security of {} failed (Win32 {code})",
            folder.display()
        ));
    }
    let inherit = SUB_CONTAINERS_AND_OBJECTS_INHERIT;
    // SAFETY: both ACLs are parts of the live descriptor.
    let standing = unsafe {
        has_entry(dacl, ACCESS_ALLOWED_ACE_TYPE, inherit, GRANT_MASK, sid)
            && has_entry(
                dacl,
                ACCESS_DENIED_ACE_TYPE,
                CONTAINER_INHERIT_ACE,
                FILE_DELETE_CHILD,
                &world,
            )
            && has_entry(
                sacl,
                SYSTEM_MANDATORY_LABEL_ACE_TYPE,
                inherit,
                SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
                &low,
            )
    };
    let applied = if standing {
        Ok(())
    } else {
        let entries = [
            entry(
                &world,
                DENY_ACCESS,
                FILE_DELETE_CHILD,
                CONTAINER_INHERIT_ACE,
            ),
            entry(sid, GRANT_ACCESS, GRANT_MASK, inherit),
        ];
        let label = low_label(&low)?;
        let mut merged: *mut ACL = null_mut();
        // SAFETY: two entries whose SIDs outlive the call, the old ACL from the live descriptor.
        let code = unsafe { SetEntriesInAclW(2, entries.as_ptr(), dacl, &raw mut merged) };
        if code == 0 {
            // SAFETY: a NUL terminated name and two valid ACLs.
            let code = unsafe {
                SetNamedSecurityInfoW(
                    name.as_mut_ptr(),
                    SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | LABEL_SECURITY_INFORMATION,
                    null_mut(),
                    null_mut(),
                    merged,
                    label.as_ptr().cast(),
                )
            };
            // SAFETY: the block `SetEntriesInAclW` allocated, freed once.
            unsafe { LocalFree(merged.cast()) };
            (code == 0)
                .then_some(())
                .ok_or_else(|| format!("granting {} failed (Win32 {code})", folder.display()))
        } else {
            Err(format!("SetEntriesInAclW failed (Win32 {code})"))
        }
    };
    // SAFETY: the descriptor the read allocated, freed once, after its last use.
    unsafe { LocalFree(descriptor) };
    applied
}
