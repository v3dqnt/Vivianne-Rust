# shell/src/rail

The project rail beside a chat, with Overview, Files, Preview and Terminal tabs.

- `mod.rs`: the `Rail` view and its tabs.
  - Overview: folder, branch, last commit and the changed files (click one to preview it).
  - Files: an expandable tree with git change marks.
  - Preview: numbered text, rendered markdown or an image, with Copy path.
  - Terminal: starts a shell the first time the tab opens.
- `files.rs`: the tree and the file reader/classifier. No UI types, so it is unit tested.
- `git.rs`: branch, last commit and `git status`, parsed. No UI types.
- `terminal/`: the shell view. See its README.
