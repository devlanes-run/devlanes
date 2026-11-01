# devlanes-hosts-helper

## 0.10.0 — 2026-11-01

### Patch changes

- [b620780](https://github.com/devlanes-run/devlanes/commit/b62078026d18be567d5e1d238f89840a8aa2778b) Fixed every devlanes update asking to reinstall the /etc/hosts helper; it now asks only when the helper itself changed.
  
  `devlanes doctor` and the hosts status now report an old helper as `helper_revision_outdated` instead of `helper_version_mismatch`.

## 0.4.0 ... 0.9.0 — 2026-10-29

Version bump only, to follow other packages.

