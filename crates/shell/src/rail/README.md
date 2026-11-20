# shell/src/rail

The project rail beside a chat, with Overview, Files, Preview and Terminal tabs.

- `mod.rs`: the `Rail` view and its tabs.
  - Overview: folder, branch, last commit and the changed files (click one to preview it).
  - Context card: tokens used of the model's window, split by what fills it (`context.rs`).
  - Last run card: the files the newest run changed, with Undo (`checkpoints` in the runtime crate).
  - Dev server card: starts the project's dev server and offers its address (`tools::dev_server`).
  - Files: an expandable tree with git change marks.
  - Preview: numbered text, rendered markdown or an image, with Copy path and Open in (an editor on this machine, the default app, or the folder).
  - Terminal: starts a shell the first time the tab opens.
- `files.rs`: the tree and the file reader/classifier. No UI types, so it is unit tested.
- `git.rs`: branch, last commit and `git status`, parsed. No UI types.
- `terminal/`: the shell view. See its README.
