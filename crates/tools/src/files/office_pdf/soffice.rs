//! The engine: a LibreOffice found on this machine, run headless in a private folder for each conversion.
//!
//! The original converts through an independently published LibreOffice kit (native or WASM) that ships with the app.
//! There is no such package for Rust, so this uses an installed `soffice`; it neither downloads one nor looks beyond the
//! usual install places. Without one, every conversion fails as `unavailable`.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use tokio_util::sync::CancellationToken;

use super::output::read_pdf;
use super::types::{Code, Converted, Converter, Error, Extension};

/// The environment variable that names the executable explicitly.
pub const EXECUTABLE_ENV: &str = "VIV_SOFFICE";

/// Where an installed LibreOffice is looked for, in order: the environment, then `PATH`, then the usual places.
#[must_use]
pub fn find() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os(EXECUTABLE_ENV).map(PathBuf::from) {
        return explicit.is_file().then_some(explicit);
    }
    let names: &[&str] = if cfg!(windows) {
        &["soffice.exe"]
    } else {
        &["soffice", "libreoffice"]
    };
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in names {
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    let usual: &[&str] = if cfg!(windows) {
        &[
            "C:\\Program Files\\LibreOffice\\program\\soffice.exe",
            "C:\\Program Files (x86)\\LibreOffice\\program\\soffice.exe",
        ]
    } else if cfg!(target_os = "macos") {
        &["/Applications/LibreOffice.app/Contents/MacOS/soffice"]
    } else {
        &[
            "/usr/bin/soffice",
            "/usr/lib/libreoffice/program/soffice",
            "/opt/libreoffice/program/soffice",
            "/snap/bin/libreoffice",
        ]
    };
    usual.iter().map(PathBuf::from).find(|p| p.is_file())
}

/// A `file:` URL for a folder, as LibreOffice's `-env:UserInstallation` wants it.
#[must_use]
pub fn file_url(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    if text.starts_with('/') {
        format!("file://{text}")
    } else {
        format!("file:///{text}")
    }
}

/// The arguments of one headless conversion of `input` into `out_dir`, with its own settings folder so two conversions
/// never share (or lock) a user profile.
#[must_use]
pub fn arguments(profile: &Path, out_dir: &Path, input: &Path) -> Vec<String> {
    vec![
        "--headless".into(),
        "--norestore".into(),
        "--nologo".into(),
        "--nodefault".into(),
        "--nolockcheck".into(),
        format!("-env:UserInstallation={}", file_url(profile)),
        "--convert-to".into(),
        "pdf".into(),
        "--outdir".into(),
        out_dir.to_string_lossy().into_owned(),
        input.to_string_lossy().into_owned(),
    ]
}

/// An installed LibreOffice.
#[derive(Debug, Clone)]
pub struct Soffice {
    executable: PathBuf,
    timeout: Duration,
    max_output_bytes: usize,
}

impl Soffice {
    /// `timeout_ms` counts from the moment the engine starts, not from queueing.
    #[must_use]
    pub fn new(executable: PathBuf, timeout_ms: u64, max_output_bytes: usize) -> Self {
        Self {
            executable,
            timeout: Duration::from_millis(timeout_ms),
            max_output_bytes,
        }
    }
}

#[async_trait]
impl Converter for Soffice {
    async fn convert(
        &self,
        bytes: &[u8],
        extension: Extension,
        cancel: &CancellationToken,
    ) -> Result<Converted, Error> {
        if cancel.is_cancelled() {
            return Err(Error::new(Code::Cancelled, "Office conversion cancelled"));
        }
        let directory = std::env::temp_dir().join(format!(
            "viv-office-to-pdf-{}",
            uuid::Uuid::new_v4().simple()
        ));
        let result = self.run(&directory, bytes, extension, cancel).await;
        // The folder goes whether the conversion worked or not, before the caller hears about it.
        let _ = std::fs::remove_dir_all(&directory);
        result
    }
}

impl Soffice {
    async fn run(
        &self,
        directory: &Path,
        bytes: &[u8],
        extension: Extension,
        cancel: &CancellationToken,
    ) -> Result<Converted, Error> {
        let failed = |detail: String| {
            Error::new(Code::Failed, "LibreOffice conversion failed.").with_detail(detail)
        };
        let profile = directory.join("profile");
        std::fs::create_dir_all(&profile).map_err(|e| failed(e.to_string()))?;
        let input = directory.join(format!("source.{}", extension.as_str()));
        {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&input)
                .map_err(|e| failed(e.to_string()))?;
            file.write_all(bytes).map_err(|e| failed(e.to_string()))?;
        }
        let mut command = crate::proc::quiet(&self.executable);
        command
            .args(arguments(&profile, directory, &input))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let child = command.spawn().map_err(|e| {
            Error::new(Code::Unavailable, "LibreOffice could not be started.")
                .with_detail(e.to_string())
        })?;
        let waited = tokio::select! {
            () = cancel.cancelled() => return Err(Error::new(Code::Cancelled, "Office conversion cancelled")),
            waited = tokio::time::timeout(self.timeout, child.wait_with_output()) => waited,
        };
        let output = match waited {
            Err(_) => return Err(Error::new(Code::Timeout, "The conversion took too long.")),
            Ok(Err(e)) => return Err(failed(e.to_string())),
            Ok(Ok(output)) => output,
        };
        if !output.status.success() {
            return Err(failed(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ));
        }
        let pdf = read_pdf(&directory.join("source.pdf"), self.max_output_bytes)?;
        // The engine reports no missing fonts here: binary Office formats never do, and OOXML detection needs the kit.
        Ok(Converted {
            pdf,
            missing_fonts: Vec::new(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_folder_is_given_as_a_file_url_on_every_platform() {
        assert_eq!(file_url(Path::new("/tmp/p")), "file:///tmp/p");
        assert_eq!(
            file_url(Path::new("C:\\Users\\a b\\p")),
            "file:///C:/Users/a b/p"
        );
    }

    #[test]
    fn the_command_is_headless_private_and_names_the_input_last() {
        let args = arguments(
            Path::new("/tmp/x/profile"),
            Path::new("/tmp/x"),
            Path::new("/tmp/x/source.docx"),
        );
        assert!(
            args.contains(&"--headless".to_owned()) && args.contains(&"--norestore".to_owned())
        );
        assert!(args.contains(&"-env:UserInstallation=file:///tmp/x/profile".to_owned()));
        let at = args
            .iter()
            .position(|a| a == "--convert-to")
            .expect("convert-to");
        assert_eq!(args[at + 1], "pdf");
        assert_eq!(args.last().map(String::as_str), Some("/tmp/x/source.docx"));
    }

    #[tokio::test]
    async fn an_engine_that_cannot_start_is_unavailable_and_leaves_no_scratch_folder() {
        let engine = Soffice::new(PathBuf::from("definitely-not-a-real-soffice"), 1000, 1000);
        let before = scratch_folders();
        let error = engine
            .convert(b"x", Extension::Docx, &CancellationToken::new())
            .await
            .unwrap_err();
        assert_eq!(error.code, Code::Unavailable);
        assert_eq!(scratch_folders(), before);
    }

    #[tokio::test]
    async fn a_cancelled_conversion_never_starts_the_engine() {
        let engine = Soffice::new(PathBuf::from("definitely-not-a-real-soffice"), 1000, 1000);
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert_eq!(
            engine
                .convert(b"x", Extension::Docx, &cancel)
                .await
                .unwrap_err()
                .code,
            Code::Cancelled
        );
    }

    fn scratch_folders() -> usize {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else {
            return 0;
        };
        entries
            .flatten()
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .starts_with("viv-office-to-pdf-")
            })
            .count()
    }
}
