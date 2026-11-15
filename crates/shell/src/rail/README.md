# shell/src/rail

The project rail beside a chat, with Overview, Files, Preview and Terminal tabs.

- `mod.rs`: `Rail` view. Overview shows the folder and git branch; Files is a real expandable tree; clicking a
  file opens it in Preview. Terminal is a placeholder (the terminal spike works, the view is not built).
- `files.rs`: the tree and the preview reader. No UI types, so it is unit tested.
