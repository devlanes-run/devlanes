//! `devlanes-hosts` binary; all logic is in the library so it can be tested.

#[cfg(unix)]
fn main() -> std::process::ExitCode {
    devlanes_hosts_helper::cli::main()
}

/// Refuses to run, since the helper edits `/etc/hosts` through sudo. devlanes
/// writes no hosts file on Windows, so the helper is not shipped there.
#[cfg(windows)]
fn main() -> std::process::ExitCode {
    eprintln!("devlanes-hosts: not supported on Windows");
    std::process::ExitCode::from(devlanes_hosts_helper::exit::USAGE)
}
