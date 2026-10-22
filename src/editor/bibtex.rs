//! Locate a BibTeX entry by its citation key.
//!
//! The offset is **char-indexed**, matching the rest of the editor contract.
//! Entries start with `@type{key,` or `@type(key,`; whitespace after the
//! opening delimiter is allowed, and `@` signs inside field values are ignored.

/// The character offset of the entry whose key matches `key`.
///
/// Keys compare case-insensitively, as BibTeX citations do. Returns `None`
/// when no entry carries the key.
#[must_use]
pub fn bibtex_entry_offset(text: &str, key: &str) -> Option<usize> {
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
        let entry_key: String = rest[open + 1..]
            .trim_start()
            .chars()
            .take_while(|character| {
                *character != ','
                    && *character != '}'
                    && *character != ')'
                    && !character.is_whitespace()
            })
            .collect();
        if entry_key.eq_ignore_ascii_case(key) {
            return Some(text[..at].chars().count());
        }
    }
    None
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
}
