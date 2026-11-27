//! Starting the command under the restricted token with the runner's own standard streams, inside a job that ends
//! with the runner.
//!
//! Port of the inherited-stdio path of `win32-process`: suspended, assigned to the job, then resumed. No new console:
//! a restricted child with one fails to load.

use std::ffi::OsString;
use std::ptr::{null, null_mut};

use windows_sys::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows_sys::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
    SetInformationJobObject,
};
use windows_sys::Win32::System::Threading::{
    CREATE_SUSPENDED, CreateProcessAsUserW, GetExitCodeProcess, INFINITE, PROCESS_INFORMATION,
    ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOW, TerminateProcess, WaitForSingleObject,
};

use super::token::{Owned, last_error};

/// One argument as the C runtime will read it back.
fn quote(argument: &str, line: &mut String) {
    if !argument.is_empty() && !argument.contains([' ', '\t', '\n', '\u{b}', '"']) {
        line.push_str(argument);
        return;
    }
    line.push('"');
    let mut backslashes = 0;
    for c in argument.chars() {
        match c {
            '\\' => backslashes += 1,
            '"' => {
                // The backslashes before a quote are doubled, and the quote escaped.
                line.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                line.push('"');
                backslashes = 0;
            }
            other => {
                line.extend(std::iter::repeat_n('\\', backslashes));
                line.push(other);
                backslashes = 0;
            }
        }
    }
    line.extend(std::iter::repeat_n('\\', backslashes * 2));
    line.push('"');
}

fn command_line(command: &[OsString]) -> Result<Vec<u16>, String> {
    let mut line = String::new();
    for argument in command {
        let argument = argument
            .to_str()
            .ok_or("the command is not valid Unicode")?;
        if argument.contains('\0') {
            return Err("the command holds a NUL".to_owned());
        }
        if !line.is_empty() {
            line.push(' ');
        }
        quote(argument, &mut line);
    }
    Ok(line.encode_utf16().chain([0]).collect())
}

fn kill_on_close_job() -> Result<Owned, String> {
    // SAFETY: an unnamed job with default security.
    let job = unsafe { CreateJobObjectW(null(), null()) };
    if job.is_null() {
        return Err(last_error("CreateJobObjectW"));
    }
    let job = Owned(job);
    // SAFETY: all-zero is a valid value of this plain structure.
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    // SAFETY: the structure and its size match the information class.
    let set = unsafe {
        SetInformationJobObject(
            job.0,
            JobObjectExtendedLimitInformation,
            (&raw const limits).cast(),
            size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if set == 0 {
        return Err(last_error("SetInformationJobObject"));
    }
    Ok(job)
}

fn inherited(which: u32) -> HANDLE {
    // SAFETY: asks for a standard handle and marks it inheritable; a null or invalid one is passed on as it is.
    unsafe {
        let handle = GetStdHandle(which);
        SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        handle
    }
}

/// Runs `command` to its end under `token` and returns its exit code.
pub(super) fn run(token: &Owned, command: &[OsString]) -> Result<i32, String> {
    let mut line = command_line(command)?;
    let job = kill_on_close_job()?;
    // SAFETY: Ctrl+C is the child's to answer; the runner stays to clean up and report its exit.
    unsafe { SetConsoleCtrlHandler(None, 1) };
    // SAFETY: all-zero is a valid value of these plain structures.
    let (mut startup, mut info): (STARTUPINFOW, PROCESS_INFORMATION) =
        unsafe { std::mem::zeroed() };
    startup.cb = size_of::<STARTUPINFOW>() as u32;
    startup.dwFlags = STARTF_USESTDHANDLES;
    startup.hStdInput = inherited(STD_INPUT_HANDLE);
    startup.hStdOutput = inherited(STD_OUTPUT_HANDLE);
    startup.hStdError = inherited(STD_ERROR_HANDLE);
    // SAFETY: the command line is a writable NUL terminated buffer; environment and folder are the runner's own.
    let started = unsafe {
        CreateProcessAsUserW(
            token.0,
            null(),
            line.as_mut_ptr(),
            null(),
            null(),
            1,
            CREATE_SUSPENDED,
            null(),
            null_mut(),
            &startup,
            &mut info,
        )
    };
    if started == 0 {
        return Err(last_error("CreateProcessAsUserW"));
    }
    let (process, thread) = (Owned(info.hProcess), Owned(info.hThread));
    // SAFETY: handles of the process just created, still suspended.
    unsafe {
        if AssignProcessToJobObject(job.0, process.0) == 0 {
            let why = last_error("AssignProcessToJobObject");
            // It has run nothing yet and must not run outside the job.
            TerminateProcess(process.0, 1);
            return Err(why);
        }
        if ResumeThread(thread.0) == u32::MAX {
            let why = last_error("ResumeThread");
            TerminateProcess(process.0, 1);
            return Err(why);
        }
        WaitForSingleObject(process.0, INFINITE);
    }
    let mut code = 0;
    // SAFETY: the process handle is open and `code` receives the value.
    if unsafe { GetExitCodeProcess(process.0, &mut code) } == 0 {
        return Err(last_error("GetExitCodeProcess"));
    }
    Ok(code.cast_signed())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_are_quoted_as_the_c_runtime_reads_them() {
        let quoted = |argument: &str| {
            let mut line = String::new();
            quote(argument, &mut line);
            line
        };
        assert_eq!(quoted("plain"), "plain");
        assert_eq!(quoted(""), "\"\"");
        assert_eq!(quoted("a b"), "\"a b\"");
        assert_eq!(quoted(r#"say "hi""#), r#""say \"hi\"""#);
        assert_eq!(quoted(r"C:\dir with space\"), r#""C:\dir with space\\""#);
        assert_eq!(quoted(r#"a\"b"#), r#""a\\\"b""#);
    }
}
