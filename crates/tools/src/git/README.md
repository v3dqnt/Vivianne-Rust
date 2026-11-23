# tools/src/git

What the Git tab reads and does, with no UI. Each piece runs the `git` program or calls GitHub.

- `mod.rs`: the repository's state (`Summary`, `Remote`, commits, branches), diffs per file, commit, sync, checkout,
  create branch, and what to ask a model to write a commit message or pull request.
- `diff.rs`: reading a unified diff: numbered lines, hiding import-only changes, marking changed words, the
  `git apply` command.
- `github.rs`: one REST call for the folder's GitHub repository, and where the token comes from.
- `time.rs`: ages and durations ("5m ago", "1m 20s") from ISO 8601 text.
