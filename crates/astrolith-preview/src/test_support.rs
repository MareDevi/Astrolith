//! Test-only helpers: a single Tokio runtime per scenario and shell fixtures.
//!
//! Each `block_on` owns a fresh runtime for the whole scenario. A dev server must
//! be started, observed and stopped inside one runtime: the drain tasks and the
//! Tokio process reaper are bound to that runtime.

#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Run `future` to completion on a private multi-thread runtime.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("build test runtime")
        .block_on(future)
}

/// Write an executable POSIX shell script with a `#!/bin/sh` shebang.
#[cfg(unix)]
pub fn write_script(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write script");
    let mut permissions = std::fs::metadata(&path).expect("stat script").permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&path, permissions).expect("chmod script");
    path
}
