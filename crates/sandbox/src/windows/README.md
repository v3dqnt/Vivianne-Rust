# windows

The Windows confinement runner: `viv` and the app start themselves again with `--viv-sandbox-run`, lower their token and run the command under it.

- `mod.rs`: arguments, which folders may be granted, the private temporary folder, capability SIDs.
- `token.rs`: the `WRITE_RESTRICTED`, Low integrity token.
- `acl.rs`: the grant on a writable folder.
- `process.rs`: quoting, `CreateProcessAsUserW`, the kill-on-close job.

## What it changes on disk, and does not undo

The first `workspace-write` command in a project edits that folder's security, inherited by everything in it:

- an allow entry for a SID derived from the folder's path (write and delete);
- a deny of "delete subfolders and files" to Everyone, on folders (your own deletes still work through each file's own delete right; opening such a folder with full control is refused);
- a Low integrity label, so **other** Low integrity programs of yours can write there too.

There is no revoke. `icacls` shows the entries but cannot remove the capability one. A large tree takes a while the first time. `read-only` changes nothing.

A folder that holds your profile, the temporary folder, Windows or the running program is never granted. Reads, the network and other processes are not restricted. Hard links alias the file they point at.

Lock files stay in `%TEMP%\viv-acl-locks`; a runner that is killed leaves its `%TEMP%\viv-sbx-*` folder.
