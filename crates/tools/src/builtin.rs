// Copied from Vivianne's original tools (agent/tools/builtin.rs).
// Kept as close to the original as possible; do not restyle.
#![allow(clippy::all, clippy::pedantic, clippy::nursery, clippy::unwrap_used, clippy::expect_used, clippy::todo, clippy::dbg_macro, unsafe_code, missing_debug_implementations)]

use async_trait::async_trait;
use serde_json::json;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Duration;


/// Result of executing a tool.
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub details: Option<serde_json::Value>,
}

/// Trait for tool implementations.
///
/// Deliberately just `name` + `execute` — this used to also carry
/// `description()`/`parameters()`, but nothing ever called them: every
/// schema the model actually sees comes from `agent::tools::schemas::
/// static_tool_definitions()`. Two copies of every tool's schema, one of
/// them silently dead, is how they drifted (the dead copy of `bash`
/// documented a `timeout` param the live one didn't expose at all — the
/// code honored it either way, the model just couldn't discover it existed).
/// One schema, one place to keep it accurate.
#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    /// The tool's definition (`name`, `description`, `parameters`), for a tool defined outside this crate. The ones
    /// here are looked up by name instead.
    fn definition(&self) -> Option<serde_json::Value> {
        None
    }
    async fn execute(&self, args: &serde_json::Value, cwd: &str) -> ToolResult;
}

/// Factory for creating the default set of coding tools. Called fresh on
/// every `execute_tool_calls` (see that function) rather than built once at
/// startup — which is what makes playbooks (below) hot-reload for free: a
// ── Shared limits & helpers ─────────────────────────────────────────────

const MAX_GREP_MATCHES: usize = 100;
const MAX_GREP_BYTES: usize = 256_000;
const MAX_GREP_LINE_CHARS: usize = 1000;

/// Directories that are always skipped during recursive walks — shared by
/// every walker in this module (`walk_files` here, `IgnoreSet::always_skip`
/// below, `repo_tools::walk_structure`). These used to be three
/// separately-maintained lists with three different contents (`.turbo` and
/// `.cache` were only in one of them), so `grep` and `list_dir` could
/// genuinely disagree about what exists in the repo. One list now.
pub(crate) const ALWAYS_SKIP_DIRS: &[&str] = &[
    ".git", "node_modules", "target", "dist", "build", ".next", ".venv", "venv", "__pycache__",
    ".turbo", ".cache", "vendor",
];

/// Truncate `text` to at most `max_lines` lines AND `max_bytes` bytes (whichever
/// limit is hit first), UTF-8-safely, appending a notice when truncated.
pub(crate) fn truncate_text(text: &str, max_lines: usize, max_bytes: usize) -> (String, bool) {
    let lines: Vec<&str> = text.lines().collect();
    let line_truncated = lines.len() > max_lines;
    let head_lines = if line_truncated { &lines[..max_lines] } else { &lines[..] };
    let mut joined = head_lines.join("\n");

    let mut byte_truncated = false;
    if joined.len() > max_bytes {
        byte_truncated = true;
        let mut cut = max_bytes;
        // Never split a multi-byte UTF-8 char.
        while cut > 0 && !joined.is_char_boundary(cut) {
            cut -= 1;
        }
        joined.truncate(cut);
    }

    let truncated = line_truncated || byte_truncated;
    if truncated {
        joined.push_str(&format!(
            "\n\n[... output truncated ({} lines / {} bytes shown; original had {} lines) ...]",
            joined.lines().count(),
            joined.len(),
            lines.len()
        ));
    }
    (joined, truncated)
}

/// Resolve `path` against `cwd`, rejecting absolute escapes is out of scope
/// here (the agent operates within a trusted workspace) but we still
/// normalize `.` to the cwd itself.
pub(crate) fn resolve_path(cwd: &str, path: &str) -> PathBuf {
    if path.is_empty() || path == "." {
        return PathBuf::from(cwd);
    }
    Path::new(cwd).join(path)
}

/// Convert a simple glob (`*`, `**`, `?`) into an anchored regex.
///
/// `*` matches within one path segment (`[^/]*`); `**` crosses segment
/// boundaries (`.*`), the usual meaning of "at any depth". Both used to
/// compile to plain `.*`, so `glob("src/*.rs")` wrongly matched
/// `src/foo/bar.rs` — a single `*` reaching across a `/` the caller never
/// asked it to cross. `?` is one non-separator character for the same
/// reason. Returns `Result` rather than falling back to a permanently
/// non-matching regex on a compile failure — every character this function
/// emits into the pattern is already escaped or a fixed, valid snippet, so a
/// failure isn't expected in practice, but a caller that gets one should see
/// a real error instead of a search that silently, indistinguishably found
/// nothing.
pub(crate) fn glob_to_regex(glob: &str) -> Result<regex::Regex, regex::Error> {
    const REGEX_META: &str = r".+()|[]{}^$\";
    let mut out = String::from("^");
    let mut chars = glob.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '*' => {
                if chars.peek() == Some(&'*') {
                    chars.next();
                    // `**/` at any depth is the standard idiom for "this
                    // segment, or any number of segments, or none at all" —
                    // gitignore and most glob implementations treat it as
                    // matching zero directories too (`**/foo` also matches a
                    // root-level `foo`), so the trailing `/` has to be
                    // optional along with the `.*`, not a separate literal
                    // that would force at least one path segment to exist.
                    if chars.peek() == Some(&'/') {
                        chars.next();
                        out.push_str("(?:.*/)?");
                    } else {
                        out.push_str(".*");
                    }
                } else {
                    out.push_str("[^/]*");
                }
            }
            '?' => out.push_str("[^/]"),
            c if REGEX_META.contains(c) => {
                out.push('\\');
                out.push(c);
            }
            c => out.push(c),
        }
    }
    out.push('$');
    regex::Regex::new(&out)
}

/// Recursively collect files under `root`, skipping noisy directories.
/// Bounded by `limit` to avoid pathological trees; runs synchronously and is
/// always called from within `spawn_blocking`.
///
/// Returns whether the walk was cut short by `limit` — distinct from `visit`
/// returning `false` (the caller decided it already has enough, e.g. grep
/// hit its own match cap) or the tree being exhausted naturally. A caller
/// that only checked "were there more matches than I displayed" used to miss
/// this entirely: a search with few or zero matches could still silently
/// stop at file #4000 of a much larger tree, and nothing distinguished that
/// from "the whole tree really has no matches" — the model would report a
/// false negative with full confidence.
pub(crate) fn walk_files(root: &Path, limit: usize, mut visit: impl FnMut(&Path) -> bool) -> bool {
    let mut stack = vec![root.to_path_buf()];
    let mut visited = 0usize;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            if visited >= limit {
                return true;
            }
            let path = entry.path();
            let file_type = match entry.file_type() {
                Ok(t) => t,
                Err(_) => continue,
            };
            if file_type.is_dir() {
                let name = entry.file_name().to_string_lossy().to_string();
                if ALWAYS_SKIP_DIRS.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if file_type.is_file() {
                visited += 1;
                if !visit(&path) {
                    return false;
                }
            }
        }
    }
    false
}

// ── Grep ─────────────────────────────────────────────────────────────────

pub struct GrepTool {
    cwd: String,
}

impl GrepTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    async fn execute(&self, args: &serde_json::Value, _cwd: &str) -> ToolResult {
        let pattern = args["pattern"].as_str().unwrap_or("").to_string();
        let path = args["path"].as_str().unwrap_or(".").to_string();
        let search_path = resolve_path(&self.cwd, &path);

        let regex = match regex::Regex::new(&pattern) {
            Ok(r) => r,
            Err(e) => {
                return ToolResult {
                    content: format!("Invalid regex pattern: {}", e),
                    is_error: true,
                    details: None,
                };
            }
        };

        let base = PathBuf::from(&self.cwd);
        let result = tokio::task::spawn_blocking(move || -> (Vec<String>, usize, bool) {
            let mut matches = Vec::new();
            let mut total_matches = 0usize;
            let mut output_bytes = 0usize;
            let mut scan_capped = false;

            if search_path.is_file() {
                grep_one_file(&search_path, &base, &regex, &mut matches, &mut total_matches, &mut output_bytes);
            } else {
                // Capped like `code_search`'s walk — a sparse-match search on
                // a huge repo previously still touched every file under root
                // (only the match/byte counters broke out early, not the walk
                // itself).
                scan_capped = walk_files(&search_path, MAX_SEARCH_FILES_SCANNED, |file_path| {
                    grep_one_file(file_path, &base, &regex, &mut matches, &mut total_matches, &mut output_bytes);
                    total_matches < MAX_GREP_MATCHES && output_bytes < MAX_GREP_BYTES
                });
            }
            (matches, total_matches, scan_capped)
        })
        .await;

        match result {
            Ok((mut lines, total, scan_capped)) => {
                let truncated = total > lines.len();
                if truncated {
                    lines.push(format!("... {} more matches (limit {})", total - lines.len(), MAX_GREP_MATCHES));
                }
                if scan_capped {
                    // Never let a partial scan look exhaustive: this is what
                    // used to make the model confidently report "X isn't used
                    // anywhere" after silently stopping at file #4000 of a
                    // much larger tree.
                    lines.push(format!(
                        "\n[WARNING: scan stopped after {MAX_SEARCH_FILES_SCANNED} files — results are PARTIAL, \
                         not exhaustive. Narrow `path` and retry to search the rest of the tree.]"
                    ));
                }
                ToolResult {
                    content: lines.join("\n"),
                    is_error: false,
                    details: Some(json!({ "pattern": pattern, "path": path, "matches": total, "truncated": truncated, "scan_capped": scan_capped })),
                }
            }
            Err(e) => ToolResult {
                content: format!("Internal error running grep: {}", e),
                is_error: true,
                details: None,
            },
        }
    }
}

fn grep_one_file(
    path: &Path,
    base: &Path,
    regex: &regex::Regex,
    matches: &mut Vec<String>,
    total_matches: &mut usize,
    output_bytes: &mut usize,
) {
    if matches.len() >= MAX_GREP_MATCHES || *output_bytes >= MAX_GREP_BYTES {
        return;
    }
    let Ok(content) = std::fs::read_to_string(path) else { return }; // skip binary/non-UTF8
    for (idx, line) in content.lines().enumerate() {
        if !regex.is_match(line) {
            continue;
        }
        *total_matches += 1;
        if matches.len() >= MAX_GREP_MATCHES || *output_bytes >= MAX_GREP_BYTES {
            continue;
        }
        let mut display_line = line.to_string();
        if display_line.len() > MAX_GREP_LINE_CHARS {
            display_line.truncate(MAX_GREP_LINE_CHARS);
            display_line.push_str("...");
        }
        let entry = format!("{}:{}:{}", display_path(path, base), idx + 1, display_line);
        *output_bytes += entry.len();
        matches.push(entry);
    }
}

/// A path as the model should see it: relative to the workspace when it's
/// inside it (with forward slashes), absolute otherwise. An absolute Windows
/// path on every result line cost the model tens of characters per match and
/// told it nothing it didn't already know.
pub(crate) fn display_path(path: &Path, base: &Path) -> String {
    match path.strip_prefix(base) {
        Ok(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().replace('\\', "/"),
        _ => path.display().to_string(),
    }
}

/// How many files grep scans before stopping and saying its result is partial.
const MAX_SEARCH_FILES_SCANNED: usize = 4000;

// ── Code search ──────────────────────────────────────────────────────────
// Semantic + BM25 hybrid search over a per-project embedding index — see
// `agent::tools::code::semantic_index` for how it works and why.

pub struct CodeSearchTool {
    cwd: String,
}

impl CodeSearchTool {
    pub fn new(cwd: String) -> Self {
        Self { cwd }
    }
}

#[async_trait]
impl Tool for CodeSearchTool {
    fn name(&self) -> &str {
        "code_search"
    }

    async fn execute(&self, args: &serde_json::Value, _cwd: &str) -> ToolResult {
        let query = args["query"].as_str().unwrap_or("").trim().to_string();
        if crate::code::semantic_index::tokens(&query).is_empty() && query.len() < 3 {
            return ToolResult { content: "'query' must describe what to look for".to_string(), is_error: true, details: None };
        }
        let path = args["path"].as_str().unwrap_or(".").to_string();
        let root = resolve_path(&self.cwd, &path);
        if !root.is_dir() {
            return ToolResult { content: format!("Not a folder: {}", root.display()), is_error: true, details: None };
        }

        let (hits, status) = crate::code::semantic_index::search(&root, &query, 10).await;
        let mode = match status.embedder {
            Some(e) => format!("semantic ({}) + keyword", e.label()),
            None => "keyword only".to_string(),
        };
        let mut notes = Vec::new();
        if let Some(err) = &status.embed_error {
            notes.push(format!("embedding failed ({err}); ranked by keywords"));
        }
        if status.incomplete {
            notes.push("the index is still being built — results cover part of the project; search again to include the rest".to_string());
        }
        if status.embedder.is_none() && status.embed_error.is_none() {
            notes.push("no embedding key (OpenAI or NVIDIA) is set, so this is keyword ranking; exact names still work best with grep".to_string());
        }

        if hits.is_empty() {
            let mut msg = format!("No relevant code found for: {query} [{mode}, {} files indexed]", status.files);
            for n in &notes {
                msg.push_str(&format!("\n({n})"));
            }
            return ToolResult { content: msg, is_error: false, details: None };
        }

        let mut out = format!("{} results for: {query} [{mode}, {} files indexed]\n", hits.len(), status.files);
        for h in &hits {
            out.push_str(&format!("\n{}:{}-{}  (score {:.2})\n", h.path, h.start, h.end, h.score));
            for line in h.preview.lines() {
                let mut l = line.to_string();
                if l.len() > MAX_GREP_LINE_CHARS {
                    l.truncate(MAX_GREP_LINE_CHARS);
                    l.push_str("...");
                }
                out.push_str(&format!("    {l}\n"));
            }
        }
        for n in &notes {
            out.push_str(&format!("\n({n})"));
        }
        ToolResult {
            content: out.trim_end().to_string(),
            is_error: false,
            details: Some(json!({ "query": query, "results": hits.len(), "semantic": status.embedder.is_some() })),
        }
    }
}

// ═══ glob, powershell, notebooks, web, sleep, todos, diagnostics ═══

const MAX_GLOB_RESULTS: usize = 100;

pub(crate) fn err(msg: impl Into<String>) -> ToolResult {
    ToolResult { content: format!("Error: {}", msg.into()), is_error: true, details: None }
}
fn ok(content: impl Into<String>) -> ToolResult {
    ToolResult { content: content.into(), is_error: false, details: None }
}

// ── glob ───────────────────────────────────────────────────────────────────

pub struct GlobTool { cwd: String }
impl GlobTool { pub fn new(cwd: String) -> Self { Self { cwd } } }

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str { "glob" }
    async fn execute(&self, args: &serde_json::Value, _cwd: &str) -> ToolResult {
        let pattern = args["pattern"].as_str().unwrap_or("").to_string();
        if pattern.is_empty() { return err("'pattern' is required"); }
        let path = args["path"].as_str().unwrap_or(".").to_string();
        let root = resolve_path(&self.cwd, &path);
        let matcher = match glob_to_regex(&pattern) {
            Ok(r) => r,
            Err(e) => return err(format!("invalid glob pattern '{pattern}': {e}")),
        };
        let root_for_rel = root.clone();
        let base = std::path::PathBuf::from(&self.cwd);

        let result = tokio::task::spawn_blocking(move || -> (Vec<String>, usize) {
            let mut found = Vec::new();
            let mut total = 0usize;
            walk_files(&root, usize::MAX, |file_path| {
                let rel = file_path
                    .strip_prefix(&root_for_rel)
                    .unwrap_or(file_path)
                    .to_string_lossy()
                    .replace('\\', "/");
                let name = file_path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                if matcher.is_match(&rel) || matcher.is_match(&name) {
                    total += 1;
                    if found.len() < MAX_GLOB_RESULTS {
                        found.push(display_path(file_path, &base));
                    }
                }
                total < MAX_GLOB_RESULTS * 8
            });
            (found, total)
        }).await;

        match result {
            Ok((lines, total)) => {
                if lines.is_empty() { return ok("No files found"); }
                let mut out = lines.join("\n");
                if total > lines.len() {
                    out.push_str(&format!("\n(Results truncated: showing {} of {}+ — use a more specific pattern.)", lines.len(), total));
                }
                ok(out)
            }
            Err(e) => err(format!("glob failed: {e}")),
        }
    }
}

// ── diagnostics ──────────────────────────────────────────────────────────────

pub struct DiagnosticsTool { cwd: String }
impl DiagnosticsTool { pub fn new(cwd: String) -> Self { Self { cwd } } }

#[async_trait]
impl Tool for DiagnosticsTool {
    fn name(&self) -> &str { "diagnostics" }
    async fn execute(&self, args: &serde_json::Value, _cwd: &str) -> ToolResult {
        let path = args["path"].as_str().unwrap_or("");
        if path.is_empty() { return err("'path' is required"); }
        let full_path = resolve_path(&self.cwd, path);
        if !full_path.is_file() {
            return err(format!("{path} is not a file"));
        }
        let full = full_path.display().to_string();

        // A language server first: it knows the whole project (types, imports).
        let lsp = crate::lsp::manager::report(&full, &self.cwd, Duration::from_secs(15)).await;
        if let crate::lsp::manager::Report::Published(items) = &lsp {
            return match crate::lsp::format_diagnostics(&full, items) {
                Some(block) => ok(block),
                None => ok(format!("No errors in {path} (checked by its language server).")),
            };
        }

        // No server, or it didn't answer in time: the language's own tools.
        let root = std::path::Path::new(&self.cwd);
        if let Some(result) = crate::code::checkers::check_file(&full_path, root).await {
            return if result.problems.is_empty() {
                ok(format!("No errors in {path} (checked with {}).", result.checker))
            } else {
                let mut out = format!("{} problem(s) in {path} (checked with {}):\n", result.problems.len(), result.checker);
                for p in result.problems.iter().take(30) {
                    out.push_str(&format!("ERROR [{p}]\n"));
                }
                ok(out.trim_end().to_string())
            };
        }

        // Nothing could check it — say so rather than implying it's clean.
        match lsp {
            crate::lsp::manager::Report::NotReported => ok(format!(
                "Couldn't check {path}: its language server is still starting and hadn't reported after 15s. Try again in a moment, or run the project's build/lint command."
            )),
            _ => ok(format!(
                "Couldn't check {path}: no language server or checker is available for this file type. Run the project's own build or lint command to verify it."
            )),
        }
    }
}

// ── Gitignore helper ─────────────────────────────────────────────────────────
//
// Vivianne doesn't take a hard dep on the `ignore` crate; the cost of the
// binary is not worth it for the simple case here. Instead we read `.gitignore`
// at the search root and honor the small subset actually used in real repos:
// blank lines, `#` comments, `!` negation, trailing `/` (directory only),
// leading `/` (root-anchored), and simple `*` / `?` globs.

#[derive(Debug, Clone)]
struct IgnoreRule {
    pattern: regex::Regex,
    negate: bool,
    dir_only: bool,
}

#[derive(Debug, Clone, Default)]
pub struct IgnoreSet {
    rules: Vec<IgnoreRule>,
    /// Always-skipped directory names (regardless of `.gitignore`).
    always_skip: HashSet<String>,
}

impl IgnoreSet {
    pub fn from_root(root: &Path) -> Self {
        let mut rules = Vec::new();
        for name in [".gitignore", ".ignore"] {
            let path = root.join(name);
            let Ok(content) = std::fs::read_to_string(&path) else { continue };
            for raw in content.lines() {
                let line = raw.trim();
                if line.is_empty() || line.starts_with('#') { continue; }
                let mut pattern = line.to_string();
                let negate = pattern.starts_with('!');
                if negate { pattern.remove(0); }
                let dir_only = pattern.ends_with('/');
                if dir_only { pattern.pop(); }
                let anchored = pattern.starts_with('/');
                if anchored { pattern.remove(0); }
                let regex_src = build_ignore_regex(&pattern, anchored);
                if let Ok(re) = regex::Regex::new(&regex_src) {
                    rules.push(IgnoreRule { pattern: re, negate, dir_only });
                }
            }
        }
        // One skip-list, shared with `tool_exec::walk_files` — this used to
        // be a second, independently-maintained set (missing "vendor",
        // present here but absent there) so `grep` and `list_dir` could
        // genuinely disagree about what exists in the repo.
        let always_skip: HashSet<String> =
            ALWAYS_SKIP_DIRS.iter().map(|s| s.to_string()).collect();
        Self { rules, always_skip }
    }

    /// `rel` is the path relative to the root (forward-slash separated).
    /// `is_dir` distinguishes trailing-slash `dir/` patterns.
    pub fn is_ignored(&self, rel: &str, is_dir: bool) -> bool {
        // First: always-skip directories.
        if is_dir {
            if let Some(name) = rel.rsplit('/').next() {
                if self.always_skip.contains(name) { return true; }
            }
            if self.always_skip.contains(rel) { return true; }
        }
        let mut ignored = false;
        for r in &self.rules {
            if r.dir_only && !is_dir { continue; }
            if r.pattern.is_match(rel) {
                ignored = !r.negate;
            }
        }
        ignored
    }
}

fn build_ignore_regex(pattern: &str, anchored: bool) -> String {
    const META: &str = r".+()|[]{}^$\";
    let mut out = String::new();
    if anchored { out.push('^'); } else { out.push_str("(^|.*/)"); }
    let mut prev_star = false;
    for c in pattern.chars() {
        match c {
            '*' => {
                if prev_star {
                    // `**` — match across path segments.
                    out.push_str(".*");
                    prev_star = false;
                } else {
                    prev_star = true;
                }
            }
            '?' => {
                if prev_star { out.push_str("[^/]*"); prev_star = false; }
                out.push_str("[^/]");
            }
            c => {
                if prev_star { out.push_str("[^/]*"); prev_star = false; }
                if META.contains(c) { out.push('\\'); }
                out.push(c);
            }
        }
    }
    if prev_star { out.push_str("[^/]*"); }
    out.push_str("($|/.*$)");
    out
}

// ── read ─────────────────────────────────────────────────────────────────────

pub struct ReadTool { cwd: String }
impl ReadTool { pub fn new(cwd: String) -> Self { Self { cwd } } }

const MAX_READ_LINES: usize = 2000;
const MAX_READ_BYTES: usize = 50_000;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str { "read" }

    async fn execute(&self, args: &serde_json::Value, _cwd: &str) -> ToolResult {
        let path = args["path"].as_str().unwrap_or("").to_string();
        let full = resolve_path(&self.cwd, &path);

        // Existence check.
        let Ok(meta) = tokio::fs::metadata(&full).await else {
            return ToolResult {
                content: format!("Error reading file: {} not found", path),
                is_error: true,
                details: None,
            };
        };
        if meta.is_dir() {
            return ToolResult {
                content: format!("{} is a directory. Use list_dir or ls.", path),
                is_error: true,
                details: None,
            };
        }

        let ext = full.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();

        // ── Jupyter notebook ──
        if ext == "ipynb" {
            let text = match tokio::fs::read_to_string(&full).await {
                Ok(t) => t,
                Err(e) => return ToolResult { content: format!("Error reading notebook: {e}"), is_error: true, details: None },
            };
            let parsed: serde_json::Value = match serde_json::from_str(&text) {
                Ok(v) => v,
                Err(e) => return ToolResult { content: format!("Notebook is not valid JSON: {e}"), is_error: true, details: None },
            };
            return read_notebook_summary(&path, parsed);
        }

        // ── Image ──
        if is_image_ext(&ext) {
            let do_ocr = args.get("ocr").and_then(|v| v.as_bool()).unwrap_or(false);
            if do_ocr {
                return read_image(path.clone(), full.clone(), true).await;
            }
            return read_image(path.clone(), full.clone(), false).await;
        }

        // ── PDF ──
        if ext == "pdf" {
            let pages = args["pages"].as_str().unwrap_or("").to_string();
            return read_pdf_summary(&path, &full, &pages).await;
        }

        // ── Word, Excel, PowerPoint ──
        // Their text, part by part, rather than a hexdump of the zip: an agent
        // that wrote a document must be able to read it back to check it.
        if matches!(ext.as_str(), "docx" | "xlsx" | "pptx") {
            let p = full.clone();
            return match tokio::task::spawn_blocking(move || crate::files::documents::inspect(&p)).await {
                Ok(Ok(parts)) => {
                    let filled: Vec<_> = parts.iter().filter(|x| !x.text.trim().is_empty()).collect();
                    let body = filled.iter().map(|x| format!("[{}] {}", x.label, x.text)).collect::<Vec<_>>().join("\n");
                    let empty = parts.len() - filled.len();
                    ToolResult {
                        content: format!(
                            "{path}: {} parts, {} with text{}\n\n{}",
                            parts.len(),
                            filled.len(),
                            if empty > 0 { format!(", {empty} empty") } else { String::new() },
                            if body.is_empty() { "(no text in this document)".to_string() } else { body }
                        ),
                        is_error: false,
                        details: Some(json!({ "path": path, "parts": parts.len(), "withText": filled.len() })),
                    }
                }
                Ok(Err(e)) => ToolResult { content: format!("Couldn't read {path}: {e}"), is_error: true, details: None },
                Err(e) => ToolResult { content: format!("Couldn't read {path}: {e}"), is_error: true, details: None },
            };
        }

        // ── Text ──
        let content = match tokio::fs::read_to_string(&full).await {
            Ok(c) => c,
            Err(_) => {
                // Not valid UTF-8: return the size + a hexdump of the first 128 bytes.
                let bytes = match tokio::fs::read(&full).await {
                    Ok(b) => b,
                    Err(e) => return ToolResult { content: format!("Error reading file: {e}"), is_error: true, details: None },
                };
                let head = bytes.iter().take(128).map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ");
                return ToolResult {
                    content: format!("Binary file: {} ({} bytes)\nHead (128 bytes hex):\n{head}", path, bytes.len()),
                    is_error: false,
                    details: Some(json!({ "path": path, "binary": true, "size": bytes.len() })),
                };
            }
        };

        let offset = args["offset"].as_u64().unwrap_or(0) as usize;
        let limit = args["limit"].as_u64().unwrap_or(0) as usize;

        let lines: Vec<&str> = content.lines().collect();
        let start = offset.saturating_sub(1).min(lines.len());
        let end = if limit > 0 { (start + limit).min(lines.len()) } else { lines.len() };
        let mut numbered = String::with_capacity(content.len().min(MAX_READ_BYTES));
        for (i, line) in lines[start..end].iter().enumerate() {
            numbered.push_str(&format!("{:>6}\t{}\n", start + i + 1, line));
        }
        let (mut result, truncated) = truncate_text(&numbered, MAX_READ_LINES, MAX_READ_BYTES);
        // `truncate_text` already appends a notice when it cuts something —
        // but the far more common case is a small file shown in full, where
        // there was previously no signal at all of the total line count.
        // The model had no way to tell "this really is the whole file" from
        // "this got silently cut somewhere I can't see" without this line.
        if !truncated {
            result.push_str(&format!("\n[{} lines total]", lines.len()));
        }
        ToolResult {
            content: result,
            is_error: false,
            details: Some(json!({
                "path": path,
                "lines": lines.len(),
                "shown": end - start,
                "truncated": truncated,
            })),
        }
    }
}

pub(crate) fn is_image_ext(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "tif" | "tiff" | "ico")
}

/// Longest edge a decoded image is downscaled to before being sent to the
/// model — Anthropic's documented ideal max edge for vision input. Without
/// this a multi-megapixel screenshot goes over the wire at full resolution
/// for no accuracy benefit, just token/bandwidth cost.
const MAX_IMAGE_EDGE: u32 = 1568;

async fn read_image(display_path: String, full: PathBuf, do_ocr: bool) -> ToolResult {
    let (w, h, fmt, bytes, image_b64, media_type, encode_err) = {
        let full_path = full.clone();
        let result = tokio::task::spawn_blocking(move || -> Result<(u32, u32, String, u64, Option<String>, &'static str, Option<String>), String> {
            let raw = std::fs::read(&full_path).map_err(|e| e.to_string())?;
            let img = image::load_from_memory(&raw).map_err(|e| e.to_string())?;
            let (w, h) = (img.width(), img.height());
            let format = image::guess_format(&raw).ok();
            let fmt = match format {
                Some(f) => format!("{:?}", f).to_lowercase(),
                None => "unknown".to_string(),
            };

            // Already small enough, and already a format the model can take
            // as-is: pass the original bytes straight through rather than
            // decoding and re-encoding as PNG. For a photo, decode+RGBA8+PNG
            // can ship several MB of lossless base64 where the original JPEG
            // was a fraction of that — a token-cost regression for the sake
            // of a re-encode nothing needed.
            let already_fine = w.max(h) <= MAX_IMAGE_EDGE
                && matches!(format, Some(image::ImageFormat::Png) | Some(image::ImageFormat::Jpeg));
            if already_fine {
                use base64::Engine;
                let media_type = if format == Some(image::ImageFormat::Jpeg) { "image/jpeg" } else { "image/png" };
                let b64 = base64::engine::general_purpose::STANDARD.encode(&raw);
                return Ok((w, h, fmt, raw.len() as u64, Some(b64), media_type, None));
            }

            let longest = w.max(h);
            let scaled = if longest > MAX_IMAGE_EDGE {
                let scale = MAX_IMAGE_EDGE as f64 / longest as f64;
                img.resize((w as f64 * scale) as u32, (h as f64 * scale) as u32, image::imageops::FilterType::Triangle)
            } else {
                img
            };

            // RGB8 unless the source actually has transparency to preserve —
            // an opaque PNG or a resized JPEG doesn't need an all-255 alpha
            // channel inflating what the PNG encoder has to compress.
            let has_alpha = scaled.color().has_alpha();
            let mut png_bytes: Vec<u8> = Vec::with_capacity((scaled.width() * scaled.height()) as usize);
            let encode_err = {
                use image::codecs::png::{CompressionType, FilterType as PngFilterType, PngEncoder};
                use image::ImageEncoder;
                let encoder = PngEncoder::new_with_quality(&mut png_bytes, CompressionType::Fast, PngFilterType::Adaptive);
                let encode_result = if has_alpha {
                    let rgba = scaled.to_rgba8();
                    encoder.write_image(rgba.as_raw(), rgba.width(), rgba.height(), image::ExtendedColorType::Rgba8)
                } else {
                    let rgb = scaled.to_rgb8();
                    encoder.write_image(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
                };
                encode_result.err().map(|e| e.to_string())
            };
            let image_b64 = if encode_err.is_none() {
                use base64::Engine;
                Some(base64::engine::general_purpose::STANDARD.encode(&png_bytes))
            } else {
                None
            };

            Ok((w, h, fmt, raw.len() as u64, image_b64, "image/png", encode_err))
        }).await;
        match result {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => return ToolResult { content: format!("Error decoding image: {e}"), is_error: true, details: None },
            Err(e) => return ToolResult { content: format!("Internal error: {e}"), is_error: true, details: None },
        }
    };

    let mut content = format!(
        "Image: {}\n  format: {}\n  size: {}×{} px\n  bytes: {}",
        display_path, fmt, w, h, bytes
    );
    if let Some(e) = &encode_err {
        content.push_str(&format!("\n  (could not re-encode this image for viewing: {e} — only this text summary is available)"));
    }

    if do_ocr {
        // Reading text out of an image is not ported yet; the model can still see the image itself.
        content.push_str("\n  OCR not available: set HUGGINGFACE_TOKEN env var for API inference, or check internet connection.");
    }

    let mut details = json!({
        "path": display_path,
        "kind": "image",
        "format": fmt,
        "width": w,
        "height": h,
        "bytes": bytes,
    });
    // `image_base64`/`media_type` are the exact keys the tool dispatcher
    // already looks for (see `computer::execute_pointer_action`'s
    // screenshot action) to attach an Image content block — no dispatch-side
    // change needed to wire this up.
    // `media_type` matches whichever path produced `image_b64`: the original
    // file's real type when passed through unmodified, "image/png" whenever
    // this function did the encoding itself.
    if let Some(b64) = image_b64 {
        details["image_base64"] = json!(b64);
        details["media_type"] = json!(media_type);
    }

    ToolResult { content, is_error: false, details: Some(details) }
}

async fn read_pdf_summary(display_path: &str, full: &PathBuf, pages: &str) -> ToolResult {
    let display = display_path.to_string();
    let full = full.clone();
    let pages = pages.to_string();
    let result = tokio::task::spawn_blocking(move || -> Result<(usize, String, u64), String> {
        let bytes = std::fs::read(&full).map_err(|e| e.to_string())?;
        if !bytes.starts_with(b"%PDF-") {
            return Err("not a PDF (missing %PDF- header)".to_string());
        }
        let size = bytes.len() as u64;
        // Cheap page count: `/Type /Page` object occurrences (not /Pages).
        // Regex on the raw bytes; this misses encrypted PDFs but doesn't crash.
        let text = String::from_utf8_lossy(&bytes).to_string();
        let re = regex::Regex::new(r"/Type\s*/Page[^s]").unwrap();
        let page_count = re.find_iter(&text).count();
        // Best-effort text: extract every parenthesized string inside a `BT ... ET`
        // block. This is a real PDF text extractor's toy version — hits the
        // common case of "text in a form-XObject" without decoding CMAP fonts.
        let mut extracted = String::new();
        let bt = regex::Regex::new(r"(?s)BT\b(.*?)\bET").unwrap();
        let tj = regex::Regex::new(r"\((?:\\.|[^\\)])*\)").unwrap();
        for cap in bt.captures_iter(&text) {
            let block = &cap[1];
            for m in tj.find_iter(block) {
                let s = &m.as_str()[1..m.as_str().len() - 1];
                extracted.push_str(&s.replace("\\(", "(").replace("\\)", ")"));
                extracted.push(' ');
            }
            extracted.push('\n');
        }
        let extracted = extracted.trim().to_string();

        // Page filter: keep the requested slice of extracted output when `pages`
        // is set. Since our extractor isn't page-aware, we approximate by
        // splitting on double-newline as a page separator.
        let extracted = if pages.is_empty() {
            extracted
        } else {
            let ranges = parse_page_range(&pages, page_count);
            let all_pages: Vec<&str> = extracted.split("\n\n").collect();
            let mut buf = String::new();
            for (i, p) in all_pages.iter().enumerate() {
                if ranges.contains(&(i + 1)) {
                    buf.push_str(p);
                    buf.push_str("\n\n");
                }
            }
            buf.trim().to_string()
        };
        Ok((page_count, extracted, size))
    }).await;
    match result {
        Ok(Ok((pages_n, text, size))) => {
            let (clipped, truncated) = truncate_text(&text, MAX_READ_LINES, MAX_READ_BYTES);
            let body = if clipped.trim().is_empty() {
                "(no text extracted — PDF may be image-only, encrypted, or use custom fonts)".to_string()
            } else { clipped };
            ToolResult {
                content: format!(
                    "PDF: {}\n  pages: {pages_n}\n  bytes: {size}\n  extracted text:\n\n{body}",
                    display
                ),
                is_error: false,
                details: Some(json!({
                    "path": display,
                    "kind": "pdf",
                    "pages": pages_n,
                    "bytes": size,
                    "truncated": truncated,
                })),
            }
        }
        Ok(Err(e)) => ToolResult { content: format!("Error reading PDF: {e}"), is_error: true, details: None },
        Err(e) => ToolResult { content: format!("Internal error: {e}"), is_error: true, details: None },
    }
}

fn parse_page_range(spec: &str, total: usize) -> Vec<usize> {
    let mut out = Vec::new();
    for part in spec.split(',') {
        let part = part.trim();
        if let Some((a, b)) = part.split_once('-') {
            let a: usize = a.trim().parse().unwrap_or(1);
            let b: usize = b.trim().parse().unwrap_or(total);
            for i in a..=b.min(total) { out.push(i); }
        } else if let Ok(n) = part.parse::<usize>() {
            out.push(n);
        }
    }
    out
}

fn read_notebook_summary(display_path: &str, nb: serde_json::Value) -> ToolResult {
    let cells = nb.get("cells").and_then(|c| c.as_array()).cloned().unwrap_or_default();
    let mut out = format!("Jupyter notebook: {}\n  cells: {}\n", display_path, cells.len());
    let mut ncode = 0usize;
    let mut nmd = 0usize;
    for (i, cell) in cells.iter().enumerate() {
        let cell_type = cell.get("cell_type").and_then(|v| v.as_str()).unwrap_or("");
        if cell_type == "code" { ncode += 1; } else if cell_type == "markdown" { nmd += 1; }
        let src = cell.get("source");
        let text = match src {
            Some(serde_json::Value::String(s)) => s.clone(),
            Some(serde_json::Value::Array(arr)) => arr.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>().join(""),
            _ => String::new(),
        };
        let head_line = text.lines().next().unwrap_or("").trim();
        let short = if head_line.len() > 100 {
            format!("{}...", &head_line[..100])
        } else {
            head_line.to_string()
        };
        out.push_str(&format!("[{i}] {cell_type}: {short}\n"));
    }
    out.push_str(&format!("\n  code cells: {ncode}, markdown cells: {nmd}\n"));
    ToolResult {
        content: out,
        is_error: false,
        details: Some(json!({
            "path": display_path,
            "kind": "notebook",
            "cells": cells.len(),
            "code_cells": ncode,
            "markdown_cells": nmd,
        })),
    }
}

// ── ls ────────────────────────────────────────────────────────────────────────

pub struct LsTool { cwd: String }
impl LsTool { pub fn new(cwd: String) -> Self { Self { cwd } } }

const MAX_TREE_ENTRIES: usize = 1500;
const MAX_TREE_DEPTH: usize = 6;

#[async_trait]
impl Tool for LsTool {
    fn name(&self) -> &str { "ls" }

    async fn execute(&self, args: &serde_json::Value, _cwd: &str) -> ToolResult {
        let path = args["path"].as_str().unwrap_or(".").to_string();
        let depth = args["depth"].as_u64().unwrap_or(1).min(MAX_TREE_DEPTH as u64) as usize;
        let show_hidden = args["show_hidden"].as_bool().unwrap_or(false);
        let root = resolve_path(&self.cwd, &path);
        let root_for_task = root.clone();

        let result = tokio::task::spawn_blocking(move || -> Result<(String, usize, usize, bool), String> {
            let ignore = IgnoreSet::from_root(&root_for_task);
            let mut out = String::new();
            let mut counted = 0usize;
            let mut total = 0usize;
            let mut truncated = false;
            walk_tree(&root_for_task, &root_for_task, &ignore, 0, depth, show_hidden, &mut out, &mut counted, &mut total, &mut truncated);
            Ok((out, counted, total, truncated))
        }).await;

        match result {
            Ok(Ok((body, counted, total, truncated))) => {
                let mut content = format!(
                    "Listing {} (depth={}, gitignore honored, hidden={}):\n{}",
                    path, depth, show_hidden, body
                );
                if truncated {
                    content.push_str(&format!(
                        "\n[... tree truncated: showing {counted} of {total} entries; \
                         use a narrower path or lower depth ...]\n"
                    ));
                }
                ToolResult {
                    content,
                    is_error: false,
                    details: Some(json!({
                        "path": path,
                        "depth": depth,
                        "entries": counted,
                        "total": total,
                        "truncated": truncated,
                    })),
                }
            }
            Ok(Err(e)) => ToolResult { content: format!("Error listing directory: {e}"), is_error: true, details: None },
            Err(e) => ToolResult { content: format!("Internal error: {e}"), is_error: true, details: None },
        }
    }
}

fn walk_tree(
    root: &Path,
    dir: &Path,
    ignore: &IgnoreSet,
    level: usize,
    max_depth: usize,
    show_hidden: bool,
    out: &mut String,
    counted: &mut usize,
    total: &mut usize,
    truncated: &mut bool,
) {
    if *truncated { return; }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    let mut items: Vec<(std::path::PathBuf, bool, String)> = Vec::new();
    for e in entries.flatten() {
        let p = e.path();
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        let name = e.file_name().to_string_lossy().to_string();
        if !show_hidden && name.starts_with('.') { continue; }
        let rel = p.strip_prefix(root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
        if ignore.is_ignored(&rel, is_dir) { continue; }
        items.push((p, is_dir, name));
    }
    items.sort_by(|a, b| match (a.1, b.1) {
        (true, false) => std::cmp::Ordering::Less,
        (false, true) => std::cmp::Ordering::Greater,
        _ => a.2.to_ascii_lowercase().cmp(&b.2.to_ascii_lowercase()),
    });
    for (p, is_dir, name) in items {
        *total += 1;
        if *counted >= MAX_TREE_ENTRIES {
            *truncated = true;
            return;
        }
        *counted += 1;
        let indent = "  ".repeat(level);
        if is_dir {
            out.push_str(&format!("{indent}📁 {name}/\n"));
            if level + 1 < max_depth {
                walk_tree(root, &p, ignore, level + 1, max_depth, show_hidden, out, counted, total, truncated);
            }
        } else {
            let bytes = std::fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
            out.push_str(&format!("{indent}📄 {name} ({} bytes)\n", bytes));
        }
    }
}
#[cfg(test)]
mod glob_to_regex_tests {
    use super::*;

    #[test]
    fn single_star_does_not_cross_a_path_separator() {
        let re = glob_to_regex("src/*.rs").unwrap();
        assert!(re.is_match("src/foo.rs"));
        assert!(!re.is_match("src/nested/foo.rs"), "a single * must not reach across a /");
    }

    #[test]
    fn double_star_crosses_path_separators() {
        let re = glob_to_regex("**/*.ts").unwrap();
        assert!(re.is_match("foo.ts"));
        assert!(re.is_match("src/nested/deep/foo.ts"));
    }

    #[test]
    fn question_mark_matches_one_non_separator_char() {
        let re = glob_to_regex("a?.txt").unwrap();
        assert!(re.is_match("ab.txt"));
        assert!(!re.is_match("a/.txt"), "? must not match a path separator");
    }

    #[test]
    fn double_star_slash_also_matches_zero_directories() {
        // `**/*.ts` is the standard idiom for "at any depth, including the
        // root" — it must match a root-level file too, not just ones nested
        // under at least one directory.
        let re = glob_to_regex("**/*.ts").unwrap();
        assert!(re.is_match("foo.ts"));
    }
}

#[cfg(test)]
mod read_ls_tests {
    use super::*;

    #[test]
    fn gitignore_matches_directory_pattern() {
        let mut set = IgnoreSet::default();
        set.rules.push(IgnoreRule {
            pattern: regex::Regex::new(&build_ignore_regex("build", false)).unwrap(),
            negate: false,
            dir_only: true,
        });
        assert!(set.is_ignored("build", true));
        assert!(!set.is_ignored("build.rs", false));
    }

    #[test]
    fn parse_pages() {
        assert_eq!(parse_page_range("1-3", 10), vec![1, 2, 3]);
        assert_eq!(parse_page_range("2,4,6-7", 10), vec![2, 4, 6, 7]);
    }
}
