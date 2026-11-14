//! Locate a BibTeX entry by its citation key.
//!
//! Offsets are **char-indexed**, matching the rest of the editor contract.
//! Entries start with `@type{key,` or `@type(key,`; whitespace after the
//! opening delimiter is allowed, and `@` signs inside field values are ignored.

/// One `@type{key` header found in a bibliography.
struct EntryHeader {
    /// Byte offset of the `@`.
    byte_offset: usize,
    /// The same offset counted in chars.
    char_offset: usize,
    /// The citation key.
    key: String,
}

/// The character offset of the entry whose key matches `key`.
///
/// Keys compare case-insensitively, as BibTeX citations do. Returns `None`
/// when no entry carries the key.
#[must_use]
pub fn bibtex_entry_offset(text: &str, key: &str) -> Option<usize> {
    entry_headers(text)
        .into_iter()
        .find(|header| header.key.eq_ignore_ascii_case(key))
        .map(|header| header.char_offset)
}

/// Remove the entry whose key matches `key`.
///
/// The whole `@type{…}` or `@type(…)` block is removed, including the
/// whitespace around it, so removing adjacent entries leaves no blank gaps.
/// Keys compare case-insensitively. Returns `None` when no entry carries the
/// key.
#[must_use]
pub fn remove_bibtex_entry(text: &str, key: &str) -> Option<String> {
    let header = entry_headers(text)
        .into_iter()
        .find(|header| header.key.eq_ignore_ascii_case(key))?;
    let end = entry_end(text, header.byte_offset)?;
    let bytes = text.as_bytes();

    let mut start = header.byte_offset;
    while start > 0 && bytes[start - 1].is_ascii_whitespace() {
        start -= 1;
    }
    let mut end = end;
    while end < bytes.len() && bytes[end].is_ascii_whitespace() {
        end += 1;
    }

    let left = &text[..start];
    let right = &text[end..];
    let separator = match (left.is_empty(), right.is_empty()) {
        (false, false) => "\n\n",
        (false, true) => "\n",
        _ => "",
    };
    Some(format!("{left}{separator}{right}"))
}

/// The byte offset just past the closing delimiter of the entry at `at`.
fn entry_end(text: &str, at: usize) -> Option<usize> {
    let rest = &text[at + 1..];
    let open = at + 1 + rest.find(['{', '('])?;
    let open_char = text.as_bytes()[open] as char;
    let in_parentheses = open_char == '(';
    let mut braces = 0usize;
    let mut in_quotes = false;
    let mut escaped = false;

    for (offset, character) in text[open..].char_indices() {
        let position = open + offset;
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' => escaped = true,
            '{' => braces += 1,
            '}' => {
                if braces > 0 {
                    braces -= 1;
                    if braces == 0 && !in_parentheses {
                        return Some(position + 1);
                    }
                } else if !in_parentheses {
                    return Some(position + 1);
                }
            }
            '"' if braces == 0 => in_quotes = !in_quotes,
            ')' if in_parentheses && braces == 0 && !in_quotes => {
                return Some(position + 1);
            }
            _ => {}
        }
    }
    None
}

/// The 1-based line and the entry key at `byte_offset`.
///
/// A parser reports the byte span of a syntax error. This turns it into a
/// human-readable position: the line number and the key of the nearest entry
/// header at or before the offset. The key is `None` when the error precedes
/// the first entry (for example in a broken preamble).
///
/// `byte_offset` clamps to the end of `text`.
#[cfg(feature = "grobid")]
#[must_use]
pub(crate) fn bibtex_entry_location(text: &str, byte_offset: usize) -> (usize, Option<String>) {
    let offset = byte_offset.min(text.len());
    let line = text.as_bytes()[..offset]
        .iter()
        .filter(|byte| **byte == b'\n')
        .count()
        + 1;
    let key = entry_headers(text)
        .into_iter()
        .take_while(|header| header.byte_offset <= offset)
        .last()
        .map(|header| header.key);
    (line, key)
}

/// Every parseable entry header, in source order.
fn entry_headers(text: &str) -> Vec<EntryHeader> {
    let mut headers = Vec::new();
    let mut search = 0;
    while let Some(relative) = text[search..].find('@') {
        let at = search + relative;
        search = at + 1;
        let line_start = text[..at].rfind('\n').map_or(0, |index| index + 1);
        if !text[line_start..at].trim().is_empty() {
            continue;
        }
        let rest = &text[at + 1..];
        let Some(open) = rest.find(['{', '(']) else {
            break;
        };
        let entry_type = rest[..open].trim();
        if entry_type.is_empty()
            || !entry_type
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            continue;
        }
        let key: String = rest[open + 1..]
            .trim_start()
            .chars()
            .take_while(|character| {
                *character != ','
                    && *character != '}'
                    && *character != ')'
                    && !character.is_whitespace()
            })
            .collect();
        headers.push(EntryHeader {
            byte_offset: at,
            char_offset: text[..at].chars().count(),
            key,
        });
    }
    headers
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_found_by_key() {
        let text =
            "@article{Kano2025,\n  title = {Gaze},\n}\n\n@book{Mila2023,\n  title = {Sclera},\n}\n";
        assert_eq!(bibtex_entry_offset(text, "Kano2025"), Some(0));
        let second = text.find("@book").expect("second entry");
        assert_eq!(
            bibtex_entry_offset(text, "mila2023"),
            Some(text[..second].chars().count())
        );
        assert_eq!(bibtex_entry_offset(text, "Kano"), None);
        assert_eq!(bibtex_entry_offset(text, "Kano2025b"), None);
    }

    #[test]
    fn whitespace_and_parentheses_are_allowed() {
        let text = "  @book (\n    Key2024 ,\n  title = {Title},\n)\n";
        assert_eq!(bibtex_entry_offset(text, "Key2024"), Some(2));
    }

    #[test]
    fn an_at_sign_inside_a_field_is_not_an_entry() {
        let text = "@article{A,\n  note = {mail me @ home},\n}\n@article{B,\n}\n";
        let second = text.find("@article{B").expect("second entry");
        assert_eq!(
            bibtex_entry_offset(text, "B"),
            Some(text[..second].chars().count())
        );
    }

    #[test]
    fn offsets_are_char_indexed() {
        let text = "é\n@article{Kano2025,\n}\n";
        assert_eq!(bibtex_entry_offset(text, "Kano2025"), Some(2));
    }

    #[test]
    fn entries_are_removed_without_leaving_blank_gaps() {
        let text =
            "@article{A,\n  title = {One},\n}\n\n@book{B,\n  title = {Two},\n}\n\n@misc{C,\n}\n";
        let without_b = remove_bibtex_entry(text, "b").expect("entry B");
        assert_eq!(
            without_b,
            "@article{A,\n  title = {One},\n}\n\n@misc{C,\n}\n"
        );

        let without_a = remove_bibtex_entry(text, "A").expect("entry A");
        assert_eq!(without_a, "@book{B,\n  title = {Two},\n}\n\n@misc{C,\n}\n");

        let without_c = remove_bibtex_entry(text, "C").expect("entry C");
        assert_eq!(
            without_c,
            "@article{A,\n  title = {One},\n}\n\n@book{B,\n  title = {Two},\n}\n"
        );
    }

    #[test]
    fn removal_handles_parentheses_and_braced_values() {
        let text = "@book (A,\n  title = {A (nested) value},\n)\n@misc{B,\n}\n";
        assert_eq!(
            remove_bibtex_entry(text, "A").expect("entry A"),
            "@misc{B,\n}\n"
        );
    }

    #[test]
    fn removal_reports_a_missing_key() {
        let text = "@article{A,\n}\n";
        assert_eq!(remove_bibtex_entry(text, "Missing"), None);
    }

    #[cfg(feature = "grobid")]
    #[test]
    fn locations_name_the_line_and_the_entry() {
        let text =
            "@article{Kano2025,\n  title = {Gaze},\n}\n\n@book{Mila2023,\n  title = {Sclera}\n";
        let second = text.find("@book").expect("second entry");
        assert_eq!(
            bibtex_entry_location(text, second + 3),
            (5, Some("Mila2023".to_owned()))
        );
        assert_eq!(
            bibtex_entry_location(text, 6),
            (1, Some("Kano2025".to_owned()))
        );
        assert_eq!(
            bibtex_entry_location(text, usize::MAX),
            (7, Some("Mila2023".to_owned()))
        );
        assert_eq!(bibtex_entry_location("junk\n@", 0), (1, None));
    }
}
