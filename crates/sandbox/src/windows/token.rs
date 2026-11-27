//! SIDs and the restricted token.
//!
//! Port of `token.ts`: the same restricting list, flags, integrity level and default DACL.

use std::ffi::c_void;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSidToSidW, EXPLICIT_ACCESS_W, GRANT_ACCESS, NO_MULTIPLE_TRUSTEE, SetEntriesInAclW,
    TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::{
    ACL, CopySid, CreateRestrictedToken, CreateWellKnownSid, DISABLE_MAX_PRIVILEGE, GetLengthSid,
    GetTokenInformation, PSID, SECURITY_MAX_SID_SIZE, SID_AND_ATTRIBUTES, SetTokenInformation,
    TOKEN_ADJUST_DEFAULT, TOKEN_ASSIGN_PRIMARY, TOKEN_DEFAULT_DACL, TOKEN_DUPLICATE, TOKEN_GROUPS,
    TOKEN_INFORMATION_CLASS, TOKEN_MANDATORY_LABEL, TOKEN_QUERY, TokenDefaultDacl, TokenGroups,
    TokenIntegrityLevel, WELL_KNOWN_SID_TYPE, WRITE_RESTRICTED, WinLowLabelSid, WinWorldSid,
};
use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
use windows_sys::Win32::System::SystemServices::{SE_GROUP_INTEGRITY, SE_GROUP_LOGON_ID};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

pub(super) fn wide(text: &std::ffi::OsStr) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt as _;
    text.encode_wide().chain([0]).collect()
}

pub(super) fn last_error(call: &str) -> String {
    // SAFETY: reads this thread's last error code.
    let code = unsafe { GetLastError() };
    format!("{call} failed (Win32 {code})")
}

/// A SID in memory this value owns, aligned for the structure it holds.
pub(super) struct Sid(Vec<u32>);

impl Sid {
    pub(super) fn as_ptr(&self) -> PSID {
        self.0.as_ptr().cast_mut().cast::<c_void>()
    }

    fn blank() -> Self {
        Self(vec![0; (SECURITY_MAX_SID_SIZE as usize).div_ceil(4)])
    }

    /// A copy of the SID at `from`.
    ///
    /// # Safety
    /// `from` must point to a valid SID.
    unsafe fn copy(from: PSID) -> Result<Self, String> {
        let sid = Self::blank();
        // SAFETY: the destination holds the largest SID there is; the caller vouches for the source.
        if unsafe { CopySid(SECURITY_MAX_SID_SIZE, sid.as_ptr(), from) } == 0 {
            return Err(last_error("CopySid"));
        }
        Ok(sid)
    }

    pub(super) fn parse(text: &str) -> Result<Self, String> {
        let name = wide(text.as_ref());
        let mut parsed: PSID = null_mut();
        // SAFETY: `name` is NUL terminated and `parsed` receives a block freed below.
        if unsafe { ConvertStringSidToSidW(name.as_ptr(), &mut parsed) } == 0 {
            return Err(last_error("ConvertStringSidToSidW"));
        }
        // SAFETY: the call succeeded, so `parsed` is a valid SID until it is freed.
        let sid = unsafe { Self::copy(parsed) };
        // SAFETY: the block the conversion allocated, freed once.
        unsafe { LocalFree(parsed) };
        sid
    }

    fn well_known(kind: WELL_KNOWN_SID_TYPE) -> Result<Self, String> {
        let sid = Self::blank();
        let mut size = SECURITY_MAX_SID_SIZE;
        // SAFETY: the buffer is `size` bytes long.
        if unsafe { CreateWellKnownSid(kind, null_mut(), sid.as_ptr(), &mut size) } == 0 {
            return Err(last_error("CreateWellKnownSid"));
        }
        Ok(sid)
    }

    /// Everyone.
    pub(super) fn world() -> Result<Self, String> {
        Self::well_known(WinWorldSid)
    }

    /// The Low integrity level.
    pub(super) fn low() -> Result<Self, String> {
        Self::well_known(WinLowLabelSid)
    }

    fn byte_length(&self) -> u32 {
        // SAFETY: `self` holds a valid SID.
        unsafe { GetLengthSid(self.as_ptr()) }
    }
}

/// A handle closed when dropped.
pub(super) struct Owned(pub(super) HANDLE);

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: the handle is owned by this value and closed once.
        unsafe { CloseHandle(self.0) };
    }
}

/// An access entry for `sid`. The result borrows the SID: it must not outlive it.
pub(super) fn entry(sid: &Sid, mode: i32, rights: u32, inheritance: u32) -> EXPLICIT_ACCESS_W {
    EXPLICIT_ACCESS_W {
        grfAccessPermissions: rights,
        grfAccessMode: mode,
        grfInheritance: inheritance,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: null_mut(),
            MultipleTrusteeOperation: NO_MULTIPLE_TRUSTEE,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: sid.as_ptr().cast::<u16>(),
        },
    }
}

/// One variable-length token fact, in a buffer aligned for its structure.
fn token_facts(token: HANDLE, class: TOKEN_INFORMATION_CLASS) -> Result<Vec<u64>, String> {
    let mut needed = 0;
    // SAFETY: a size query: no buffer, so only `needed` is written.
    unsafe { GetTokenInformation(token, class, null_mut(), 0, &mut needed) };
    if needed == 0 {
        return Err(last_error("GetTokenInformation size"));
    }
    let mut buffer = vec![0_u64; (needed as usize).div_ceil(8)];
    // SAFETY: the buffer holds at least `needed` bytes.
    let read = unsafe {
        GetTokenInformation(
            token,
            class,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    };
    if read == 0 {
        return Err(last_error("GetTokenInformation"));
    }
    Ok(buffer)
}

fn logon_sid(token: HANDLE) -> Result<Sid, String> {
    let facts = token_facts(token, TokenGroups)?;
    let groups = facts.as_ptr().cast::<TOKEN_GROUPS>();
    // SAFETY: the buffer is a TOKEN_GROUPS followed by `GroupCount` entries, as the call filled it.
    let entries = unsafe {
        std::slice::from_raw_parts(
            (&raw const (*groups).Groups).cast::<SID_AND_ATTRIBUTES>(),
            (*groups).GroupCount as usize,
        )
    };
    let logon = SE_GROUP_LOGON_ID as u32;
    let found = entries
        .iter()
        .find(|group| group.Attributes & logon == logon)
        .ok_or("this token has no logon SID")?;
    // SAFETY: the SID lives in `facts`, which is still alive.
    unsafe { Sid::copy(found.Sid) }
}

/// Objects the child creates (pipes among them) must be open to its restricting SIDs, or they are closed to it.
fn extend_default_dacl(token: HANDLE, sid: &Sid) -> Result<(), String> {
    let facts = token_facts(token, TokenDefaultDacl)?;
    // SAFETY: the buffer is a TOKEN_DEFAULT_DACL as the call filled it.
    let current = unsafe { (*facts.as_ptr().cast::<TOKEN_DEFAULT_DACL>()).DefaultDacl };
    if current.is_null() {
        return Err("the token has no default DACL to extend".to_owned());
    }
    let added = entry(sid, GRANT_ACCESS, FILE_ALL_ACCESS, 0);
    let mut merged: *mut ACL = null_mut();
    // SAFETY: one entry, a valid old ACL (in `facts`), and `merged` receives a block freed below.
    let code = unsafe { SetEntriesInAclW(1, &added, current, &mut merged) };
    if code != 0 {
        return Err(format!("SetEntriesInAclW failed (Win32 {code})"));
    }
    let info = TOKEN_DEFAULT_DACL {
        DefaultDacl: merged,
    };
    // SAFETY: `info` and the ACL it points to are valid for the call, which copies them.
    let set = unsafe {
        SetTokenInformation(
            token,
            TokenDefaultDacl,
            (&raw const info).cast(),
            size_of::<TOKEN_DEFAULT_DACL>() as u32,
        )
    };
    let failed = (set == 0).then(|| last_error("SetTokenInformation default DACL"));
    // SAFETY: the block `SetEntriesInAclW` allocated, freed once.
    unsafe { LocalFree(merged.cast()) };
    failed.map_or(Ok(()), Err)
}

/// This process's token, restricted: writes need an allow for the logon SID, Everyone or one of `write` as well as
/// the usual one, and the Low integrity level keeps them to folders labelled Low.
pub(super) fn restricted(write: &[Sid]) -> Result<Owned, String> {
    let mut own: HANDLE = null_mut();
    let rights = TOKEN_QUERY | TOKEN_DUPLICATE | TOKEN_ADJUST_DEFAULT | TOKEN_ASSIGN_PRIMARY;
    // SAFETY: the pseudo handle of this process; `own` receives a handle owned just below.
    if unsafe { OpenProcessToken(GetCurrentProcess(), rights, &mut own) } == 0 {
        return Err(last_error("OpenProcessToken"));
    }
    let own = Owned(own);
    let (logon, world, low) = (logon_sid(own.0)?, Sid::world()?, Sid::low()?);
    let restricting: Vec<SID_AND_ATTRIBUTES> = [&logon, &world]
        .into_iter()
        .chain(write)
        .map(|sid| SID_AND_ATTRIBUTES {
            Sid: sid.as_ptr(),
            Attributes: 0,
        })
        .collect();
    let mut token: HANDLE = null_mut();
    // SAFETY: `restricting` and the SIDs it points to outlive the call.
    let made = unsafe {
        CreateRestrictedToken(
            own.0,
            DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED,
            0,
            null(),
            0,
            null(),
            restricting.len() as u32,
            restricting.as_ptr(),
            &mut token,
        )
    };
    if made == 0 {
        return Err(last_error("CreateRestrictedToken"));
    }
    let token = Owned(token);
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: low.as_ptr(),
            Attributes: SE_GROUP_INTEGRITY as u32,
        },
    };
    let size = size_of::<TOKEN_MANDATORY_LABEL>() as u32 + low.byte_length();
    // SAFETY: `label` and the SID it points to are valid for the call.
    if unsafe {
        SetTokenInformation(
            token.0,
            TokenIntegrityLevel,
            (&raw const label).cast(),
            size,
        )
    } == 0
    {
        return Err(last_error("SetTokenInformation integrity level"));
    }
    // The temp SID is last; without writable folders the default objects stay open to Everyone.
    extend_default_dacl(token.0, write.last().unwrap_or(&world))?;
    Ok(token)
}
