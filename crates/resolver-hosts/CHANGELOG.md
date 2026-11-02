# devlanes-resolver-hosts

## 0.11.1 ... 0.11.2 — 2026-11-02

Version bump only, to follow other packages.

## 0.11.0 — 2026-11-01

### Patch changes

- [e52b808](https://github.com/devlanes-run/devlanes/commit/e52b8085584cd0224b93fb3dc97642ba299e9ef2) Fixed `devlanes setup` failing to install the /etc/hosts helper on machines whose sudo configuration already had an unrelated problem, such as GitHub's Ubuntu runners; it now installs the helper and warns about the other file.

## 0.10.0 — 2026-11-01

### Patch changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Fixed every devlanes update asking to reinstall the /etc/hosts helper; it now asks only when the helper itself changed.
  
  `devlanes doctor` and the hosts status now report an old helper as `helper_revision_outdated` instead of `helper_version_mismatch`.
- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Removed `devlanes certs trust` and `certs untrust`; use `devlanes setup`, which works out what this machine needs and asks once.
  
  Run `devlanes setup` instead of `certs trust --hosts --resolver`, and `devlanes setup --remove` instead of `certs untrust --hosts --resolver`.
  The TUI offers to run it when HTTPS or hostnames need it.

## 0.4.0 ... 0.9.0 — 2026-10-29

Version bump only, to follow other packages.

