//! Ownership of a process and everything it starts, so ending it ends all of it. On Windows the child is put in a job
//! object that kills its members when it is closed; on Unix the child leads its own process group.
//!
//! On Windows the child joins the job just after it starts, so a grandchild started in that first instant could be
//! outside it. Nothing in Vivianne relies on the job as a security boundary.

#![allow(
    unsafe_code,
    reason = "job objects and process groups are only reachable through the OS calls"
)]

use tokio::process::{Child, Command};

/// Sets up a command so its processes can be owned. Call before spawning.
pub fn prepare(command: &mut Command) {
    #[cfg(unix)]
    command.process_group(0);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        command.creation_flags(CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP);
    }
}

#[cfg(windows)]
mod platform {
    use std::ptr;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    #[derive(Debug)]
    pub struct Tree {
        job: HANDLE,
    }

    // SAFETY: a job handle may be used from any thread; the OS serialises operations on it.
    unsafe impl Send for Tree {}
    // SAFETY: as above, and no method mutates the handle itself.
    unsafe impl Sync for Tree {}

    impl Tree {
        pub fn adopt(child: &tokio::process::Child) -> std::io::Result<Self> {
            let process = child
                .raw_handle()
                .ok_or_else(|| std::io::Error::other("the child already ended"))?;
            // SAFETY: both arguments are null, which the call accepts (an unnamed job, default security).
            let job = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let tree = Self { job };
            // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a valid value of this plain C struct.
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                .map_err(std::io::Error::other)?;
            // SAFETY: the job is open, and the pointer and size describe `limits`, which outlives the call.
            let ok = unsafe {
                SetInformationJobObject(
                    tree.job,
                    JobObjectExtendedLimitInformation,
                    ptr::from_ref(&limits).cast(),
                    size,
                )
            };
            if ok == 0 {
                return Err(std::io::Error::last_os_error());
            }
            // SAFETY: the job is open and `process` is the live handle tokio holds for the child.
            if unsafe { AssignProcessToJobObject(tree.job, process.cast()) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(tree)
        }

        /// Windows has no polite signal to a tree: a console program gets nothing until it is ended.
        pub fn ask_to_stop(&self) {}

        pub fn kill(&self) {
            // SAFETY: the job handle is open for as long as `self` lives.
            unsafe { TerminateJobObject(self.job, 1) };
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            // SAFETY: the handle came from CreateJobObjectW and is closed once, here. Closing it ends its processes.
            unsafe { CloseHandle(self.job) };
        }
    }
}

#[cfg(unix)]
mod platform {
    #[derive(Debug)]
    pub struct Tree {
        group: libc::pid_t,
    }

    impl Tree {
        pub fn adopt(child: &tokio::process::Child) -> std::io::Result<Self> {
            let pid = child
                .id()
                .ok_or_else(|| std::io::Error::other("the child already ended"))?;
            Ok(Self {
                group: libc::pid_t::try_from(pid).map_err(std::io::Error::other)?,
            })
        }

        fn signal(&self, signal: libc::c_int) {
            // SAFETY: `kill` with a negative pid signals the process group the child was made leader of; a group that
            // is gone just returns an error, which does not matter here.
            unsafe { libc::kill(-self.group, signal) };
        }

        pub fn ask_to_stop(&self) {
            self.signal(libc::SIGTERM);
        }

        pub fn kill(&self) {
            self.signal(libc::SIGKILL);
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            self.signal(libc::SIGKILL);
        }
    }
}

pub use platform::Tree;

/// Takes ownership of a just-spawned child's tree.
pub fn adopt(child: &Child) -> std::io::Result<Tree> {
    Tree::adopt(child)
}
