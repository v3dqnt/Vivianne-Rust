# shell/src/git

The Git tab: the project's repository on one page (the old app's Git view). Reads and actions are in
`vivianne_tools::git`; this folder draws them.

- `mod.rs`: `GitView`: header (branch, pull request, quick actions), tab bar with counts, History, polling.
- `changes.rs`: the Changes tab: changed files with their diffs, unified or split, view options.
- `hub.rs`: the GitHub tabs: pull requests, Actions (runs, jobs, logs), issues, releases.
- `compose.rs`: the commit and pull request dialogs, with the text written from the changes by the model.
- `status.rs`: this branch's pull request and the counts GitHub gives, for the title bar and the tab badges.
- `ui.rs`: rows, marks, the action button with its progress, and reading GitHub's JSON.
