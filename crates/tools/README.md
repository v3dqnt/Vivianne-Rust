# vivianne-tools

The `Tool` trait, the `Toolbox`, and the first tools: `shell`, `read`, `write`, `edit`. All are
confined to a `Workspace`; paths outside it are refused (asking the user comes with the consent UI).
Shell has a timeout and keeps the tail of long output. Writes are atomic; edits need exactly one match.

Not ported yet: `dip` desktop control, browser, web, LSP, memory tools.
