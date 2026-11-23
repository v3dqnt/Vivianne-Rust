//! Shared helpers: a scratch folder that is removed afterwards, and a made-up file system for the failure paths.

#![allow(dead_code, clippy::expect_used, clippy::unwrap_used, clippy::pedantic)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use vivianne_context::instructions::config::{Config, Resolved};
use vivianne_context::instructions::files::{Fs, Probe};

pub struct Scratch(pub PathBuf);

impl Scratch {
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("viv-context-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("scratch");
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// Writes a file, making its folders.
    pub fn write(&self, relative: &str, content: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("folders");
        std::fs::write(&path, content).expect("write");
        path
    }

    pub fn mkdir(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        std::fs::create_dir_all(&path).expect("folder");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A project under `scratch/proj` with a `.git` marker, a home under `scratch/home`, an agents home under `scratch/agents`.
pub struct World {
    pub scratch: Scratch,
}

impl World {
    pub fn new(name: &str) -> Self {
        let scratch = Scratch::new(name);
        scratch.mkdir("proj/.git");
        scratch.mkdir("home");
        scratch.mkdir("agents");
        Self { scratch }
    }

    pub fn project(&self) -> PathBuf {
        self.scratch.path().join("proj")
    }

    pub fn resolved(&self, config: Config) -> Resolved {
        Resolved::new(
            config,
            &self.scratch.path().join("home"),
            false,
            &self.scratch.path().join("agents"),
            false,
        )
    }
}

/// A file system whose answers are set by the test.
#[derive(Default)]
pub struct FakeFs {
    pub probes: Mutex<HashMap<PathBuf, Probe>>,
    pub texts: Mutex<HashMap<PathBuf, Option<String>>>,
    pub markers: Mutex<HashMap<PathBuf, Result<bool, std::io::ErrorKind>>>,
}

impl FakeFs {
    pub fn file(&self, path: &Path, version: &str, text: &str) {
        self.probes.lock().unwrap().insert(
            path.to_path_buf(),
            Probe::Present {
                version: version.into(),
                size: Some(text.len() as u64),
            },
        );
        self.texts
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), Some(text.to_owned()));
    }

    pub fn unavailable(&self, path: &Path) {
        self.probes
            .lock()
            .unwrap()
            .insert(path.to_path_buf(), Probe::Unavailable);
    }
}

impl Fs for FakeFs {
    fn probe(&self, path: &Path) -> Probe {
        self.probes
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .unwrap_or(Probe::Absent)
    }

    fn marker_exists(&self, path: &Path) -> std::io::Result<bool> {
        match self.markers.lock().unwrap().get(path) {
            Some(Ok(b)) => Ok(*b),
            Some(Err(kind)) => Err(std::io::Error::from(*kind)),
            None => Ok(false),
        }
    }

    fn read_text(&self, path: &Path, _max: u64) -> Option<String> {
        self.texts.lock().unwrap().get(path).cloned().flatten()
    }
}
