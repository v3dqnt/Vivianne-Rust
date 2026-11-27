#![allow(clippy::pedantic, missing_debug_implementations)] // stand-in: mirrors the original signatures
// Tool definitions, copied from Vivianne's original `schemas.rs`
// (read, ls, grep, code_search, glob, repo_clone, repo_overview, diagnostics)
// and, for `lsp`, from the 0901 backend.

pub fn definitions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "name": "read",
            "description": "Read the contents of a file. Text files are returned line-numbered (like `cat -n`), with the total line count stated so you know whether you saw the whole file or need offset/limit to continue. Handles PDFs (page count + extracted text via 'pages' param) and Jupyter notebooks (.ipynb, cell-by-cell) the same way. Images are shown to you directly (downscaled if oversized) alongside a text summary (format/dimensions) — set ocr=true to also extract any text in the image instead of just seeing it. If the current model can't take image input, an image is OCR'd automatically instead of shown, so you still get any text it contains.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path of the file" },
                    "offset": { "type": "number", "description": "Start reading at this line (1-indexed) — text files only" },
                    "limit": { "type": "number", "description": "Maximum lines to read — text files only" },
                    "pages": { "type": "string", "description": "PDF page range, e.g. \"1-5\" or \"3\" — PDF files only" },
                    "ocr": { "type": "boolean", "description": "If true, also run OCR on image files to extract literal text (requires HUGGINGFACE_TOKEN, with a local RapidOCR fallback)." }
                },
                "required": ["path"]
            }
        }),
        serde_json::json!({
            "name": "ls",
            "description": "List a directory's entries; set depth to see it as a gitignore-aware tree. Output is capped at 1500 entries — narrow the path or use glob/grep when a folder is huge.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Relative path to list" },
                    "depth": { "type": "number", "description": "Tree depth, gitignore-aware (default 1 = just this folder, max 6)" },
                    "show_hidden": { "type": "boolean", "description": "Include dotfiles in output (default false)" }
                },
                "required": []
            }
        }),
        serde_json::json!({
            "name": "grep",
            "description": "Search files for a regex pattern (Rust syntax, e.g. '\\bfn\\s+\\w+'), recursively and gitignore-aware. Returns at most 100 matching lines and stops scanning after 4000 files — results say when they were cut off, and a capped scan is PARTIAL, not proof something is absent: narrow with `path` or a more specific pattern and search the rest separately.",
            "parameters": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Regex pattern to search for" },
                    "path": { "type": "string", "description": "Directory or file to search in" }
                },
                "required": ["pattern"]
            }
        }),
        serde_json::json!({
            "name": "code_search",
            "description": "Search the codebase by meaning/relevance rather than exact text — good for loose or conceptual queries (\"where do we handle authentication\", \"rate limiting logic\") where you don't know the exact string to grep for. Ranks files by relevance. For an exact known string or regex, use grep instead.",
            "parameters": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Natural-language or keyword description of what you're looking for" },
                    "path": { "type": "string", "description": "Directory to search in (default: whole workspace)" }
                },
                "required": ["query"]
            }
        }),
        serde_json::json!({
            "name": "glob",
            "description": "Find files by name pattern or wildcard (e.g. '**/*.ts', 'src/*.rs'), gitignore-aware, searched from `path` (default: the workspace). Returns at most 100 matches; anything beyond that reported as 'Results truncated' — narrow the pattern and search again.",
            "parameters": {
                "type": "object",
                "properties": {
                    "pattern": { "type": "string", "description": "Glob pattern (supports * and ?)" },
                    "path": { "type": "string", "description": "Directory to search in" }
                },
                "required": ["pattern"]
            }
        }),
        serde_json::json!({
            "name": "repo_clone",
            "description": "Clone (or reuse a cached clone of) a remote git repository so its files can be read/searched. Accepts a git URL, 'owner/repo' GitHub shorthand, or 'host/owner/repo'.",
            "parameters": {
                "type": "object",
                "properties": {
                    "repository": { "type": "string", "description": "Git URL, 'owner/repo', or 'host/owner/repo'" },
                    "refresh": { "type": "boolean", "description": "Fetch the latest remote state into the cache" },
                    "branch": { "type": "string", "description": "Branch or ref to check out" }
                },
                "required": ["repository"]
            }
        }),
        serde_json::json!({
            "name": "repo_overview",
            "description": "Get a structured overview of a directory or cached repository: file tree, detected language/ecosystem, package manager, dependency files, and likely entrypoints.",
            "parameters": {
                "type": "object",
                "properties": {
                    "repository": { "type": "string", "description": "A repository already fetched with repo_clone" },
                    "path": { "type": "string", "description": "Directory path to inspect instead of a cached repository" },
                    "depth": { "type": "number", "description": "Max structure depth to include (1-6, default 3)" }
                }
            }
        }),
        serde_json::json!({
            "name": "diagnostics",
            "description": "Get language-server diagnostics (errors/warnings) for a file. Requires a language server for that file type to be installed and on PATH.",
            "parameters": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Path to the file to check" }
                },
                "required": ["path"]
            }
        }),
        serde_json::json!({
            "name": "lsp",
            "description": "Code navigation and checks through the file's language server: goToDefinition, goToTypeDefinition, findReferences, goToImplementation, hover (type and docs), symbols (a file's outline, or with query a search across the workspace), diagnostics (errors for one file, waiting for the server), status (which servers run) and capabilities (what this file's server can do). Choose it when text search is ambiguous (an overloaded or common name, a method on a specific type). line and column are 1-based, as shown by read; put the column on the symbol. Needs a language server for the file type on PATH; read-only.",
            "parameters": {
                "type": "object",
                "properties": {
                    "operation": { "type": "string", "enum": ["goToDefinition", "goToTypeDefinition", "findReferences", "goToImplementation", "hover", "symbols", "diagnostics", "status", "capabilities"] },
                    "path": { "type": "string", "description": "File containing the symbol, absolute or relative to the workspace (any file of the language for a workspace symbol search; not needed for status)" },
                    "line": { "type": "number", "description": "1-based line (goTo*, findReferences, hover)" },
                    "column": { "type": "number", "description": "1-based column (characters); or give symbol instead" },
                    "symbol": { "type": "string", "description": "The name on that line, instead of a column; name#2 for its second occurrence" },
                    "query": { "type": "string", "description": "symbols: search the whole workspace for this name instead of listing the file" }
                },
                "required": ["operation"]
            }
        }),
        serde_json::json!({
            "name": "job_output",
            "description": "Read a background job: output since the previous read for stream jobs, or the result of a finished final-output job.",
            "parameters": {
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "Job id returned by the tool that started the background work." },
                    "wait": { "type": "boolean", "description": "Block until the job finishes or the timeout expires; a timed-out wait leaves the job running. Defaults to false." },
                    "timeout_ms": { "type": "number", "description": "Max wait in milliseconds with wait: true. Default 30000, at most 600000." }
                },
                "required": ["job_id"]
            }
        }),
        serde_json::json!({
            "name": "job_list",
            "description": "List your background jobs (running and finished) with their ids, kinds, and statuses.",
            "parameters": { "type": "object", "properties": {} }
        }),
        serde_json::json!({
            "name": "job_kill",
            "description": "Request cancellation of a running background job.",
            "parameters": {
                "type": "object",
                "properties": {
                    "job_id": { "type": "string", "description": "Job id returned by the tool that started the background work." },
                    "reason": { "type": "string", "description": "Optional short reason, recorded in the log and forwarded to the job." }
                },
                "required": ["job_id"]
            }
        }),
        serde_json::json!({
            "name": "terminal_open",
            "description": "Create a persistent terminal session. Use this for shell or REPL state that must survive across tool calls.",
            "parameters": {
                "type": "object",
                "properties": {
                    "type": { "type": "string", "description": "Terminal backend type, \"shell\"." },
                    "name": { "type": "string", "description": "Optional display name such as \"main\" or \"gdb\"." },
                    "cwd": { "type": "string", "description": "Folder inside the project to start in." }
                },
                "required": ["type"]
            }
        }),
        serde_json::json!({
            "name": "terminal_send",
            "description": "Send text to a persistent terminal. By default Enter is submitted and the call waits for output silence, timeout, or session exit.",
            "parameters": {
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "Terminal session id returned by terminal_open or terminal_list." },
                    "text": { "type": "string", "description": "UTF-8 text to write to the terminal." },
                    "submit": { "type": "boolean", "description": "Submit Enter after text (default true). Set false for control characters or incomplete REPL input." },
                    "run_in_background": { "type": "boolean", "description": "Return a job id immediately; collect with job_output or stop with job_kill." }
                },
                "required": ["sessionId", "text"]
            }
        }),
        serde_json::json!({
            "name": "terminal_read",
            "description": "Read a bounded page of retained output from a persistent terminal without sending input.",
            "parameters": {
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "Terminal session id returned by terminal_open or terminal_list." },
                    "offset": { "type": "number", "description": "Newest-relative line offset (default 0)." },
                    "count": { "type": "number", "description": "Requested line count (default 500)." }
                },
                "required": ["sessionId"]
            }
        }),
        serde_json::json!({
            "name": "terminal_signal",
            "description": "Send an allowed signal to what is running in a persistent terminal.",
            "parameters": {
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "Terminal session id returned by terminal_open or terminal_list." },
                    "signal": { "type": "string", "enum": ["SIGINT", "SIGTERM", "SIGKILL", "SIGTSTP", "SIGHUP"], "description": "SIGINT works everywhere; to end the shell use terminal_close." }
                },
                "required": ["sessionId", "signal"]
            }
        }),
        serde_json::json!({
            "name": "terminal_close",
            "description": "Close one persistent terminal and end what runs in it.",
            "parameters": {
                "type": "object",
                "properties": {
                    "sessionId": { "type": "string", "description": "Terminal session id returned by terminal_open or terminal_list." }
                },
                "required": ["sessionId"]
            }
        }),
        serde_json::json!({
            "name": "terminal_list",
            "description": "List your persistent terminal sessions.",
            "parameters": {
                "type": "object",
                "properties": {},
                "required": []
            }
        }),
    ]
}
