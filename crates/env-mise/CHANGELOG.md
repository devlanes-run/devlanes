# devlanes-env-mise

## 0.9.0 — 2026-11-23

### Patch changes

- [7cd831d1](https://github.com/devlanes-run/devlanes/commit/7cd831d172b11c471bbeba1c4816178a97bfbf37) Changed variable names on Windows to keep the case they are written in, so Gradle's `ORG_GRADLE_PROJECT_` properties work.
  
  Names in `env show`, `--explain` and `env export` output keep their spelling (for example `Path`), so look them up case-insensitively. Restart the daemon after updating so `${{ env.X }}` in infra fields finds any spelling.

## 0.4.0 ... 0.8.0 — 2026-11-16

Version bump only, to follow other packages.

