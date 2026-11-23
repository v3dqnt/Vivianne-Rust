# vivianne-attachments

Durable images and files attached to messages. A port of the `attachment` and `attachment-local` packages of the deepseek harness, in Rust.

A saved thread keeps only **references** (`vivianne_core::attachment`): an id, a name, and facts about the stored bytes. It never holds a path, a URL or base64, so a thread file carries nothing a person would not want shared. Bytes live in this crate's store, content-addressed by SHA-256, so the same bytes are stored once and survive restarts. Nothing stored is ever deleted automatically.

## Front door

`Store::new(home, Config)`, then:

| Call | What it does |
|---|---|
| `save_image`, `save_images` | Check, normalize and store images. A batch is checked whole (count, bytes, types) and every member is prepared before any is written, so a refused member leaves nothing behind. |
| `read_image` | Read a stored image and verify its digest, type, length and size against the reference. A missing, damaged or swapped object fails with a stable code. |
| `read_image_request` | The image sized for one model route (`Target` width, height, byte budget), cached on disk, with concurrent callers sharing one transformation. |
| `save_file`, `save_file_stream`, `read_file_stream` | Files byte for byte, streamed in bounded chunks, verified on read, cancellable. |
| `admit_prompt_content`, `admit_encoded_images`, `admit_encoded_file` | Wire-form admission of base64 uploads: canonical base64 only, per-message limits that count already stored uploads. |
| `image_host_path`, `file_host_path` | Where the read-only object is on this machine, for a tool that opens it by path. |

Every call blocks: run it off the render path (R2.1). Cancel with a `Cancel` token. Failures carry a stable `Code` (`TOO_MANY_IMAGES`, `IMAGE_TOO_LARGE`, `ATTACHMENT_CORRUPT`...); a message never holds a host path or bytes.

## Images

Admission limits default to the deepseek harness's: 50 MiB per image, 20 images and 200 MiB per message, 64 million pixels, 65,535 px a side; PNG, JPEG, WebP and GIF. The size limits are read from the header before any pixel is decoded, and the decoder's own allocation is capped.

The stored form is one still frame, 8 bits per sample, no metadata, rotated upright, at most 2048 x 2048 pixels, 8192 px on the long edge and about 4 MiB. A source already in that form is stored byte for byte. Otherwise it is re-encoded on a quality ladder (85, 75, 60): JPEG for opaque images, lossy WebP for transparent ones, keeping the smallest output when no quality fits. A GIF always becomes one still frame. A shrunk image records its original size.

## Files

Stored verbatim under `files/<aa>/<sha>/<name>` (a hard link to one `file-objects` object per digest), so a model reading the saved path sees the real file name. Names from another machine are cut to their last part and made safe on every host (control characters, Windows reserved characters and device names, 255 bytes).

## Source map

| File | Role |
|---|---|
| `store.rs` | The store: limits, batch save, request versions with single-flight |
| `prepare.rs` | Limits, preparing one image, reading one back |
| `image/` | `detect` (header probe, full decode), `meta` (container scanners), `normalize`, `encode` (ladder), `request` (route versions and cache) |
| `files.rs`, `names.rs` | Verbatim files and display names |
| `objects.rs`, `layout.rs` | Durable publication (stage, fsync, hard link, read-only) and the folder layout |
| `admission.rs`, `prompt.rs` | Canonical base64 and whole-prompt admission |
| `geometry.rs`, `limiter.rs`, `error.rs` | Size maths, cancellation and the compression bound, error codes |

## Deviations from the TypeScript package

- **Files have a size cap** (default 1 GiB, `Config::max_file_bytes`), counted as bytes stream in. R3.7 requires every saved file to be capped; the original accepted any size.
- **Lossy WebP** comes from `libwebp` (the `webp` crate, R6.1); the `image` crate encodes WebP lossless only, which would leave the byte-target ladder with no effect.
- **Colour profiles are stripped, not converted.** An ICC profile is removed and the pixels are treated as sRGB; the original converted through libvips.
- **Image identity differs**, because the encoders differ: the same source does not hash to the same id as in a `~/.dsh` store.
- **Sync, blocking API** with a `Cancel` token instead of promises and `AbortSignal`.

Not ported: reference-aware garbage collection (the original defers it too), audio and video.

## Boundary and checks

- **May depend on** (R1.1, `scripts/check-boundaries.sh`): `core`.
- **State it owns** (R1.6): attached images and files under `<home>/attachments/v1`, and the disposable request-size cache under `<home>/cache/attachments`. The home folder is passed in; the crate never reads `~/.viv` itself.
- **Check it:** `cargo test -p vivianne-attachments` and `cargo clippy -p vivianne-attachments --all-targets`. Tests read no real `~/.viv` and need no network (R7.3).
