Office to PDF (port of the original `office-to-pdf`): converts authorized DOC, DOCX, XLS, XLSX, PPT and PPTX files to complete, reusable PDFs.

- `types.rs`: the request, the result, failure classes (`Code`), the limits (`Config`), and the two seams (`Source`, `Converter`).
- `state.rs`, `ordered.rs`: what the queue keeps and a use-ordered map.
- `queue.rs`: bounded admission before any byte is read, one conversion shared by readers of the same bytes, background vs foreground priority, a content-addressed PDF cache with a bounded source index, cancellation and disposal.
- `output.rs`: reads only a bounded, regular, complete PDF from the engine's private folder.
- `soffice.rs`: the engine, an installed LibreOffice run headless in a private folder per conversion (`VIV_SOFFICE` names it explicitly).
- `tool.rs`: the `convert_to_pdf` tool the model calls (offered only when a LibreOffice is installed).
- `mod.rs`: `OfficeToPdf` (`convert`, `render_file`, `dispose`).
- `tests.rs`: the queue and converter with a made-up engine and sources.

Deviations from the original: the engine is a system LibreOffice (the npm kit has no Rust counterpart), so missing fonts are not reported and `unavailable` is what a machine without LibreOffice gets; there is no browser Remote method, so `render_file` takes a path the caller has already authorized; no font, archive or image settings.
