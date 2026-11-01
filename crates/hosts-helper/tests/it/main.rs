//! Integration tests of `devlanes-hosts`.

// `--hosts-file` exists only in debug builds, and the helper is Unix only; devlanes
// writes no hosts file on Windows.
#[cfg(all(debug_assertions, unix))]
mod binary;
mod revision;
