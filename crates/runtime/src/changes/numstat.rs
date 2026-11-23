//! Parsing of `git diff-tree --numstat -z` output.

/// One `--numstat` record; paths are slash-separated and relative to the repository root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: String,
    /// The path before a detected rename.
    pub old_path: Option<String>,
    pub added: usize,
    pub deleted: usize,
    pub binary: bool,
}

/// Parses NUL-terminated records. A rename record has an empty path followed by the old and the new path.
/// An error means malformed or truncated output.
pub fn parse(output: &str) -> Result<Vec<Entry>, String> {
    let mut queue: std::collections::VecDeque<&str> = output.split('\0').collect();
    if queue.pop_back() != Some("") {
        return Err("numstat output is not NUL-terminated".into());
    }
    let mut entries = Vec::new();
    while let Some(record) = queue.pop_front() {
        // Only the first two tabs separate fields; a file name keeps its own tabs.
        let malformed = || format!("malformed numstat record: {record}");
        let first = record.find('\t').ok_or_else(malformed)?;
        let second = record[first + 1..]
            .find('\t')
            .map(|i| i + first + 1)
            .ok_or_else(malformed)?;
        let (added, deleted, path) = (
            &record[..first],
            &record[first + 1..second],
            &record[second + 1..],
        );
        let (target, old_path) = if path.is_empty() {
            let old = queue.pop_front().ok_or("malformed numstat rename record")?;
            let new = queue.pop_front().ok_or("malformed numstat rename record")?;
            (new, Some(old.to_owned()))
        } else {
            (path, None)
        };
        let binary = added == "-";
        let count = |text: &str| {
            if binary {
                Ok(0)
            } else {
                text.parse::<usize>().map_err(|_| malformed())
            }
        };
        entries.push(Entry {
            path: target.to_owned(),
            old_path,
            added: count(added)?,
            deleted: count(deleted)?,
            binary,
        });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_binary_and_renamed_records_parse() {
        let out = "3\t1\ta.txt\0-\t-\timg.png\u{0}0\t0\t\0old name.txt\0new name.txt\0";
        let got = parse(out).expect("parses");
        assert_eq!(
            got[0],
            Entry {
                path: "a.txt".into(),
                old_path: None,
                added: 3,
                deleted: 1,
                binary: false
            }
        );
        assert!(got[1].binary && got[1].added == 0);
        assert_eq!(
            (got[2].path.as_str(), got[2].old_path.as_deref()),
            ("new name.txt", Some("old name.txt"))
        );
    }

    #[test]
    fn a_tab_in_a_file_name_stays_in_the_name() {
        assert_eq!(
            parse("1\t2\ta\tb.txt\0").expect("parses")[0].path,
            "a\tb.txt"
        );
    }

    #[test]
    fn truncated_output_is_an_error_not_a_guess() {
        assert!(parse("3\t1\ta.txt").is_err());
        assert!(parse("3\ta.txt\0").is_err());
        assert!(parse("x\t1\ta.txt\0").is_err());
        assert!(parse("0\t0\t\0only-old\0").is_err());
        assert_eq!(parse("").expect("no output is no entries"), []);
    }
}
