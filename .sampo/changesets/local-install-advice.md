---
cargo/devlanes: patch
cargo/devlanes-update: patch
---

Changed devlanes installed as a project dependency to skip release checks; `devlanes update` and `devlanes doctor` say to update it in the project's package.json, pyproject.toml or requirements file.
