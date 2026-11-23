//! The icon and colour of a file or folder in the Files tab, picked by what it is: its extension, a name that means
//! something (`Cargo.toml`, `.github`, `src`), or nothing special.

/// A glyph and the colour (`0xRRGGBB`) to draw it in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Look {
    /// The icon's file in the asset catalog.
    pub icon: &'static str,
    pub color: u32,
}

const fn look(icon: &'static str, color: u32) -> Look {
    Look { icon, color }
}

const FOLDER: u32 = 0x007d_8fa8;
const PLAIN: u32 = 0x008b_93a1;
const GIT: u32 = 0x00f0_5133;
const DATA: u32 = 0x00cb_cb41;
const CONFIG: u32 = 0x009a_a5b8;
const SHELL: u32 = 0x0089_e051;
const IMAGE: u32 = 0x00a0_74c4;
const DOC: u32 = 0x0051_9aba;

/// What a folder looks like. `open` picks the open glyph for a folder that has no special one.
#[must_use]
pub fn folder(name: &str, open: bool) -> Look {
    match name.to_ascii_lowercase().as_str() {
        ".git" => look("icons/folder-git.svg", GIT),
        ".github" | ".gitlab" => look("icons/folder-git-2.svg", GIT),
        "src" | "source" | "lib" | "app" => look("icons/folder-code.svg", 0x0042_a5f5),
        "crates" | "packages" | "modules" | "apps" => look("icons/folder-tree.svg", 0x00de_a584),
        "tests" | "test" | "__tests__" | "spec" => look("icons/folder-check.svg", SHELL),
        ".claude" | ".vscode" | ".idea" | "config" | ".config" | ".cargo" => {
            look("icons/folder-cog.svg", CONFIG)
        }
        "node_modules" | "target" | "dist" | "build" | "out" | ".next" | "vendor"
        | "__pycache__" => look("icons/folder-archive.svg", 0x0058_5f6b),
        "docs" | "doc" | "documentation" => look("icons/folder-open.svg", DOC),
        "scripts" | "bin" | "tools" => look("icons/folder-code.svg", SHELL),
        "assets" | "images" | "img" | "public" | "static" | "media" => {
            look("icons/folder-heart.svg", IMAGE)
        }
        _ => look(
            if open {
                "icons/folder-open.svg"
            } else {
                "icons/folder.svg"
            },
            FOLDER,
        ),
    }
}

/// What a file looks like.
#[must_use]
pub fn file(name: &str) -> Look {
    let lower = name.to_ascii_lowercase();
    // Names that mean something whatever their extension.
    match lower.as_str() {
        "package.json" | "package-lock.json" => return look("icons/file-braces.svg", 0x008c_c84b),
        "dockerfile" | "docker-compose.yml" | "docker-compose.yaml" | ".dockerignore" => {
            return look("icons/file-box.svg", 0x0024_96ed);
        }
        ".gitignore" | ".gitattributes" | ".gitmodules" => return look("icons/file-diff.svg", GIT),
        "license" | "license.md" | "license.txt" | "copying" => {
            return look("icons/file-badge.svg", 0x00d9_b45b);
        }
        "readme" | "readme.md" | "readme.txt" => return look("icons/file-text.svg", 0x0042_a5f5),
        "makefile" | "justfile" => return look("icons/file-terminal.svg", SHELL),
        "cargo.toml"
        | "cargo.lock"
        | "rust-toolchain"
        | "rust-toolchain.toml"
        | "rustfmt.toml"
        | ".rustfmt.toml"
        | "clippy.toml" => return look("icons/file-cog.svg", 0x00de_a584),
        ".env" => return look("icons/file-key.svg", DATA),
        _ => {}
    }
    if lower.starts_with(".env.") {
        return look("icons/file-key.svg", DATA);
    }
    let ext = lower.rsplit_once('.').map_or("", |(_, ext)| ext);
    match ext {
        "rs" => look("icons/file-code.svg", 0x00de_a584),
        "ts" | "tsx" | "mts" | "cts" => look("icons/file-code.svg", 0x0031_78c6),
        "js" | "mjs" | "cjs" => look("icons/file-code.svg", 0x00f1_e05a),
        "jsx" => look("icons/file-code.svg", 0x0061_dafb),
        "py" | "pyw" | "ipynb" => look("icons/file-code.svg", 0x0043_90d0),
        "go" => look("icons/file-code.svg", 0x0000_add8),
        "java" | "jar" => look("icons/file-code.svg", 0x00e7_6f00),
        "kt" | "kts" => look("icons/file-code.svg", 0x00a9_7bff),
        "swift" => look("icons/file-code.svg", 0x00f0_5138),
        "c" | "h" | "cpp" | "cc" | "hpp" => look("icons/file-code.svg", 0x0065_9ad2),
        "cs" => look("icons/file-code.svg", 0x009b_4f96),
        "rb" => look("icons/file-code.svg", 0x00cc_342d),
        "php" => look("icons/file-code.svg", 0x0088_92bf),
        "lua" => look("icons/file-code.svg", 0x0051_a0cf),
        "html" | "htm" => look("icons/file-code.svg", 0x00e3_4c26),
        "css" | "scss" | "sass" | "less" => look("icons/file-code.svg", 0x008f_6fd0),
        "vue" | "svelte" => look("icons/file-code.svg", 0x0042_b883),
        "xml" | "xaml" | "plist" => look("icons/file-code.svg", 0x00e3_7933),
        "sh" | "bash" | "zsh" | "fish" | "ps1" | "psm1" | "bat" | "cmd" => {
            look("icons/file-terminal.svg", SHELL)
        }
        "json" | "jsonc" | "json5" | "jsonl" => look("icons/file-braces.svg", DATA),
        "toml" | "yaml" | "yml" | "ini" | "cfg" | "conf" | "properties" => {
            look("icons/file-cog.svg", CONFIG)
        }
        "md" | "mdx" | "markdown" | "rst" | "txt" | "log" => look("icons/file-text.svg", DOC),
        "pdf" => look("icons/file-text.svg", 0x00e5_484d),
        "doc" | "docx" | "odt" | "rtf" => look("icons/file-text.svg", 0x002b_579a),
        "xls" | "xlsx" | "csv" | "tsv" | "ods" => look("icons/file-spreadsheet.svg", 0x0021_7346),
        "ppt" | "pptx" => look("icons/file-chart-pie.svg", 0x00d2_4726),
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "ico" | "avif" | "tiff" => {
            look("icons/file-image.svg", IMAGE)
        }
        "svg" => look("icons/file-image.svg", 0x00ff_b13b),
        "mp3" | "wav" | "flac" | "ogg" | "m4a" | "aac" => look("icons/file-music.svg", 0x0000_bcd4),
        "mp4" | "mov" | "mkv" | "webm" | "avi" => look("icons/file-video-camera.svg", 0x00ff_6b6b),
        "zip" | "tar" | "gz" | "tgz" | "7z" | "rar" | "xz" | "bz2" => {
            look("icons/file-archive.svg", 0x00c0_8d4f)
        }
        "lock" => look("icons/file-lock.svg", 0x0058_5f6b),
        "db" | "sqlite" | "sqlite3" | "sql" => look("icons/database.svg", 0x0054_9dd8),
        "wasm" | "exe" | "dll" | "so" | "dylib" | "bin" | "o" | "rlib" => {
            look("icons/file-box.svg", 0x0058_5f6b)
        }
        "ttf" | "otf" | "woff" | "woff2" => look("icons/file-type.svg", 0x00e2_6d5c),
        "pem" | "crt" | "cer" | "pub" => look("icons/file-key.svg", DATA),
        _ => look("icons/file.svg", PLAIN),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_known_by_its_name_before_its_extension() {
        assert_eq!(file("Cargo.toml").icon, "icons/file-cog.svg");
        assert_eq!(file("Cargo.toml").color, 0x00de_a584);
        assert_eq!(file("main.rs").icon, "icons/file-code.svg");
        assert_eq!(file("config.toml").icon, "icons/file-cog.svg");
        assert_eq!(file(".gitignore").icon, "icons/file-diff.svg");
        assert_eq!(file(".env.local").icon, "icons/file-key.svg");
        assert_eq!(
            file("LOGO.PNG").icon,
            "icons/file-image.svg",
            "case does not matter"
        );
        assert_eq!(file("mystery").icon, "icons/file.svg");
    }

    #[test]
    fn a_folder_is_known_by_its_name() {
        assert_eq!(folder(".github", false).icon, "icons/folder-git-2.svg");
        assert_eq!(folder("src", true).icon, "icons/folder-code.svg");
        assert_eq!(folder("whatever", true).icon, "icons/folder-open.svg");
        assert_eq!(folder("whatever", false).icon, "icons/folder.svg");
    }
}
