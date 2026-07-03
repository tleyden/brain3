use std::env;
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
