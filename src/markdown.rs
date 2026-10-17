//! Streaming-friendly Markdown helpers.

use std::borrow::Cow;

/// Closes a code fence left open at the end of a streaming answer.
///
/// A raw token stream can stop in the middle of a fenced block. Rendering that
/// text as Markdown would turn the rest of the message into a code block, so
/// the UI appends the missing closing fence to its display text. The stored
/// message body is left untouched; this only affects rendering.
///
/// Both backtick and tilde fences are recognized. A balanced document is
/// returned borrowed, so the common case allocates nothing.
pub fn stabilize_markdown(text: &str) -> Cow<'_, str> {
    let mut open: Option<&str> = None;

    for line in text.lines() {
        let trimmed = line.trim_start();
        match open {
            Some(fence) => {
                if trimmed.starts_with(fence) {
                    open = None;
                }
            }
            None if trimmed.starts_with("```") => open = Some("```"),
            None if trimmed.starts_with("~~~") => open = Some("~~~"),
            None => {}
        }
    }

    let Some(fence) = open else {
        return Cow::Borrowed(text);
    };

    let mut stabilized = text.to_owned();
    if !stabilized.ends_with('\n') {
        stabilized.push('\n');
    }
    stabilized.push_str(fence);
    Cow::Owned(stabilized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_documents_are_borrowed() {
        let text = "# Title\n\n```rust\nlet x = 1;\n```\n";
        assert!(matches!(stabilize_markdown(text), Cow::Borrowed(_)));
    }

    #[test]
    fn an_open_backtick_fence_is_closed() {
        let text = "Before\n\n```rust\nlet x = 1;";
        assert_eq!(
            stabilize_markdown(text).as_ref(),
            "Before\n\n```rust\nlet x = 1;\n```"
        );
    }

    #[test]
    fn an_open_tilde_fence_is_closed_with_the_same_fence() {
        let text = "~~~\ncode";
        assert_eq!(stabilize_markdown(text).as_ref(), "~~~\ncode\n~~~");
    }

    #[test]
    fn inline_backticks_do_not_open_a_fence() {
        let text = "Use `cargo test` here.";
        assert!(matches!(stabilize_markdown(text), Cow::Borrowed(_)));
    }

    #[test]
    fn closing_the_first_fence_leaves_the_second_open() {
        let text = "```\na\n```\n\n```\nb";
        assert_eq!(
            stabilize_markdown(text).as_ref(),
            "```\na\n```\n\n```\nb\n```"
        );
    }
}
