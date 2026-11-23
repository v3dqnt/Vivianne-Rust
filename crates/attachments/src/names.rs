//! Display names. A name given by a client may be a full path from another machine; it is cut to its last part and
//! made safe on every host, so a saved reference never leaks a folder and a stored path never escapes its folder.

const FILE_NAME_MAX_BYTES: usize = 255;

fn last_part(value: &str) -> &str {
    // Both separator styles are cut by hand: a POSIX host treats a backslash as an ordinary character, so
    // `Path::file_name` would keep a Windows client's whole local path.
    let cut = value.rfind(['/', '\\']).map_or(0, |i| i + 1);
    &value[cut..]
}

fn without_controls(value: &str) -> String {
    value.chars().filter(|c| !c.is_control()).collect()
}

fn is_windows_device(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or(name)
        .trim_end_matches(['.', ' ']);
    let lower = stem.to_ascii_lowercase();
    matches!(lower.as_str(), "con" | "prn" | "aux" | "nul")
        || ["com", "lpt"].iter().any(|p| {
            lower
                .strip_prefix(p)
                .is_some_and(|d| d.len() == 1 && matches!(d.as_bytes()[0], b'1'..=b'9'))
        })
}

fn utf8_prefix(value: &str, max_bytes: usize) -> &str {
    let mut end = 0;
    for (i, c) in value.char_indices() {
        if i + c.len_utf8() > max_bytes {
            break;
        }
        end = i + c.len_utf8();
    }
    &value[..end]
}

/// The name of a stored file: safe as a leaf name on every supported filesystem, and never empty. An absent,
/// empty or dot-only name becomes `file`.
#[must_use]
pub fn file_leaf_name(value: Option<&str>) -> String {
    let Some(value) = value else {
        return "file".to_owned();
    };
    let mut clean: String = without_controls(last_part(value))
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | ':' | '"' | '|' | '?' | '*') {
                '_'
            } else {
                c
            }
        })
        .collect();
    clean = clean.trim().trim_end_matches(['.', ' ']).to_owned();
    if is_windows_device(&clean) {
        clean.insert(0, '_');
    }
    let clean = utf8_prefix(&clean, FILE_NAME_MAX_BYTES).trim_end_matches(['.', ' ']);
    if clean.is_empty() || clean == "." || clean == ".." {
        "file".to_owned()
    } else {
        clean.to_owned()
    }
}

/// The display name of an image: its last part without control characters, at most 255 characters, or `None`.
#[must_use]
pub fn image_display_name(value: Option<&str>) -> Option<String> {
    let clean = without_controls(last_part(value?));
    let clean: String = clean.trim().chars().take(255).collect();
    (!clean.is_empty()).then_some(clean)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_names_stay_and_client_paths_of_both_styles_are_cut() {
        assert_eq!(file_leaf_name(Some("report.pdf")), "report.pdf");
        assert_eq!(
            file_leaf_name(Some("C:\\Users\\ana\\secret\\report.pdf")),
            "report.pdf"
        );
        assert_eq!(
            file_leaf_name(Some("/home/ana/../report.pdf")),
            "report.pdf"
        );
        assert_eq!(file_leaf_name(Some("a/b\\c/d.txt")), "d.txt");
    }

    #[test]
    fn controls_go_windows_characters_become_underscores_and_the_length_is_bounded() {
        assert_eq!(file_leaf_name(Some("a\u{0}b\u{7f}c.txt")), "abc.txt");
        assert_eq!(file_leaf_name(Some("a<b>c:d\"e|f?g*h")), "a_b_c_d_e_f_g_h");
        let long = file_leaf_name(Some(&"é".repeat(300)));
        assert!(long.len() <= 255 && long.chars().all(|c| c == 'é'));
    }

    #[test]
    fn trailing_dots_and_spaces_go_and_device_names_are_protected() {
        assert_eq!(file_leaf_name(Some("name. . ")), "name");
        assert_eq!(file_leaf_name(Some("con")), "_con");
        assert_eq!(file_leaf_name(Some("NUL.txt")), "_NUL.txt");
        assert_eq!(file_leaf_name(Some("com1.log")), "_com1.log");
        assert_eq!(file_leaf_name(Some("lpt9 ")), "_lpt9");
        assert_eq!(file_leaf_name(Some("com10")), "com10");
        assert_eq!(file_leaf_name(Some("console.txt")), "console.txt");
    }

    #[test]
    fn an_absent_empty_or_dot_only_name_becomes_file() {
        for name in [
            None,
            Some(""),
            Some("."),
            Some(".."),
            Some("  "),
            Some("dir/"),
            Some("\\"),
        ] {
            assert_eq!(file_leaf_name(name), "file", "{name:?}");
        }
    }

    #[test]
    fn a_leaf_name_is_a_fixed_point() {
        for name in [
            "a<b",
            "con",
            "x. ",
            "é".repeat(300).as_str(),
            "C:\\x\\y.txt",
        ] {
            let once = file_leaf_name(Some(name));
            assert_eq!(file_leaf_name(Some(&once)), once);
        }
    }

    #[test]
    fn an_image_name_keeps_its_last_part_only() {
        assert_eq!(
            image_display_name(Some("C:\\pics\\cat.png")).as_deref(),
            Some("cat.png")
        );
        assert_eq!(image_display_name(Some(" \u{1} ")), None);
        assert_eq!(image_display_name(None), None);
    }
}
