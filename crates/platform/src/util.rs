use std::env;
use std::ffi::OsStr;
use std::path::PathBuf;

/// Resolves the current user's home directory.
/// Prefers HOME for Unix compatibility and explicit overrides, then falls back
/// to USERPROFILE for default Windows environments.
pub(crate) fn user_home_dir() -> Option<PathBuf> {
    env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| env::var_os("USERPROFILE").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
}

/// Resolves the `cloudflared` executable on PATH, returning its absolute path.
/// Cross-platform: on Windows this honors PATHEXT, so native `brain3.exe`
/// runs can resolve `cloudflared.exe` without relying on a Unix `which` binary.
pub(crate) fn find_cloudflared() -> Option<PathBuf> {
    let path = env::var_os("PATH");
    find_executable_in_path("cloudflared", path.as_deref())
}

fn find_executable_in_path(binary_name: &str, paths: Option<&OsStr>) -> Option<PathBuf> {
    let cwd = env::current_dir().ok()?;
    which::which_in(binary_name, paths, cwd).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn executable_lookup_finds_cloudflared_on_explicit_search_path() {
        let temp_dir = tempfile::tempdir().expect("temp dir");
        write_fake_executable(&temp_dir.path().join("cloudflared"));
        write_fake_executable(&temp_dir.path().join("cloudflared.exe"));

        let search_path = env::join_paths([temp_dir.path()]).expect("search path");

        let found = find_executable_in_path("cloudflared", Some(&search_path))
            .expect("cloudflared should resolve");
        assert!(found.starts_with(temp_dir.path()));
        assert!(
            find_executable_in_path("definitely-not-cloudflared", Some(&search_path)).is_none()
        );
    }

    fn write_fake_executable(path: &std::path::Path) {
        std::fs::write(path, b"#!/bin/sh\nexit 0\n").expect("write fake executable");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let mut permissions = std::fs::metadata(path)
                .expect("fake executable metadata")
                .permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).expect("mark fake executable");
        }
    }
}
