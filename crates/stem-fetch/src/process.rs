//! Spawning helper programs without flashing console windows, and finding them on PATH.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command};

/// `CREATE_NO_WINDOW`: keeps console programs (yt-dlp, ffmpeg) from opening a window when
/// started from the GUI app.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// A [`Command`] that won't pop up a console window on Windows.
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

/// Kill `child` and (on Windows) the processes it started, e.g. ffmpeg under yt-dlp.
pub(crate) fn kill_tree(child: &mut Child) {
    #[cfg(windows)]
    {
        let _ = command("taskkill")
            .args(["/F", "/T", "/PID", &child.id().to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// `name` with the platform's executable suffix (`yt-dlp` -> `yt-dlp.exe` on Windows).
pub(crate) fn exe_name(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Look for an executable called `name` (without suffix) in `dir`.
pub(crate) fn in_dir(dir: &Path, name: &str) -> Option<PathBuf> {
    let p = dir.join(exe_name(name));
    is_executable(&p).then_some(p)
}

/// Look for an executable called `name` (without suffix) on PATH.
pub(crate) fn on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|d| !d.as_os_str().is_empty())
        .find_map(|d| in_dir(&d, name))
}

fn is_executable(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    if !meta.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Mark a downloaded binary as executable (no-op on Windows).
pub(crate) fn make_executable(p: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perm = std::fs::metadata(p)?.permissions();
        perm.set_mode(0o755);
        std::fs::set_permissions(p, perm)?;
    }
    #[cfg(not(unix))]
    let _ = p;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_executables_in_dir() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(in_dir(dir.path(), "tool"), None);
        let p = dir.path().join(exe_name("tool"));
        std::fs::write(&p, b"#!/bin/sh\n").unwrap();
        #[cfg(unix)]
        assert_eq!(in_dir(dir.path(), "tool"), None, "not executable yet");
        make_executable(&p).unwrap();
        assert_eq!(in_dir(dir.path(), "tool"), Some(p));
    }
}
