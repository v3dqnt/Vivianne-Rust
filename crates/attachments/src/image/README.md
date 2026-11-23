# attachments/src/image

Raster images. `detect.rs` reads a header (`probe`) or fully decodes with size limits checked first (`decode`); `meta.rs` walks the PNG, JPEG, WebP and GIF containers for metadata and extra frames without ever indexing past the input; `normalize.rs` makes the stored form; `encode.rs` is the lazy quality ladder (JPEG for opaque, lossy WebP for transparent); `request.rs` sizes a stored image for one model route and caches it.
