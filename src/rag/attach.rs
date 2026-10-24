//! Context formatting for the chat prompt.
//!
//! Ragrig's built-in strategies emit plain `[Attached: …]` blocks. The chat
//! prompt instead documents two marked containers — `source="creator"` for
//! papers the author attached and `source="corpus"` for retrieval results.
//! [`ScicoAttach`] produces exactly that markup, so the model can tell an
//! explicitly attached `@Key` paper from a retrieved chunk.
//!
//! A paper easily exceeds a local model's window: the default 4096-token
//! context leaves about 9,000 characters for the whole context block, while a
//! cited PDF can carry 100,000. [`ScicoAttach::with_context_tokens`]
//! therefore splits that budget in favour of explicit attachments and marks
//! every excerpt with the prompt's `<scico:v1:truncated …/>` element.

use ragrig::{AttachStrategy, AttachedDocument};

/// Tokens Ragrig reserves for the system prompt, transcript, and query.
const RESERVED_PROMPT_TOKENS: usize = 1024;
/// Ragrig's character-per-token estimate for prompt assembly.
const CHARS_PER_TOKEN: usize = 3;
/// Percentage of the context block reserved for explicit attachments.
const ATTACHMENT_PERCENT: usize = 70;

/// Merge attachments and retrieved chunks into Scico's context containers.
///
/// ```text
/// <scico:v1:context source="creator">
/// <scico:v1:document name="@Kano2025">…full text…</scico:v1:document>
/// </scico:v1:context>
///
/// <scico:v1:context source="corpus">
/// …retrieved passages…
/// </scico:v1:context>
/// ```
///
/// Without a budget the content passes through unchanged. With
/// [`with_context_tokens`](Self::with_context_tokens), explicit attachments
/// receive most of the context window and every shortened paper carries a
/// truncation marker.
#[derive(Clone, Debug, Default)]
pub struct ScicoAttach {
    context_tokens: Option<usize>,
}

impl ScicoAttach {
    /// Format without a character budget.
    pub fn new() -> Self {
        Self::default()
    }

    /// Keep the context block within a model window of `tokens`.
    ///
    /// Mirrors Ragrig's own retrieval budget: its fixed prompt reserve and
    /// three characters per token.
    pub fn with_context_tokens(tokens: usize) -> Self {
        Self {
            context_tokens: Some(tokens),
        }
    }

    /// The content budget in characters, when one is configured.
    pub fn budget_chars(&self) -> Option<usize> {
        self.context_tokens.map(|tokens| {
            tokens
                .saturating_sub(RESERVED_PROMPT_TOKENS)
                .saturating_mul(CHARS_PER_TOKEN)
        })
    }

    fn attachment_budget(&self) -> usize {
        self.budget_chars()
            .map_or(usize::MAX, |budget| budget * ATTACHMENT_PERCENT / 100)
    }

    fn retrieval_budget(&self, attachments: &[AttachedDocument]) -> usize {
        match self.budget_chars() {
            None => usize::MAX,
            Some(budget) if attachments.is_empty() => budget,
            Some(budget) => budget - budget * ATTACHMENT_PERCENT / 100,
        }
    }
}

impl AttachStrategy for ScicoAttach {
    fn merge(&self, retrieved: &str, attachments: &[AttachedDocument]) -> String {
        let mut context = String::new();
        if !attachments.is_empty() {
            context.push_str("<scico:v1:context source=\"creator\">\n");
            let mut remaining = self.attachment_budget();
            let share = remaining / attachments.len();
            for (index, attachment) in attachments.iter().enumerate() {
                // A pre-built protocol container (the document buffer, the
                // bibliography, a marked selection) is inserted verbatim: the
                // host already attached the attributes the prompt documents
                // (cursor, range, digest), and wrapping it again would hide
                // them inside a second `document` element.
                if is_container(&attachment.content) {
                    let content = attachment.content.trim_end();
                    remaining = remaining.saturating_sub(content.chars().count());
                    context.push_str(content);
                    context.push('\n');
                    continue;
                }
                let limit = if index + 1 == attachments.len() {
                    remaining
                } else {
                    share.min(remaining)
                };
                let total = attachment.content.chars().count();
                let (content, truncated) = truncate(&attachment.content, limit);
                remaining = remaining.saturating_sub(content.chars().count());
                context.push_str(&format!(
                    "<scico:v1:document name=\"{}\">{content}",
                    attribute(&attachment.name)
                ));
                if truncated {
                    context.push_str(&format!(
                        "<scico:v1:truncated shown=\"{}\" total=\"{total}\"/>",
                        content.chars().count()
                    ));
                }
                context.push_str("</scico:v1:document>\n");
            }
            context.push_str("</scico:v1:context>");
        }

        let retrieved = retrieved.trim_end_matches('\n');
        let (retrieved, _) = truncate(retrieved, self.retrieval_budget(attachments));
        if !retrieved.trim().is_empty() {
            if !context.is_empty() {
                context.push_str("\n\n");
            }
            context.push_str("<scico:v1:context source=\"corpus\">\n");
            context.push_str(&retrieved);
            context.push_str("\n</scico:v1:context>");
        }
        context
    }

    fn name(&self) -> &'static str {
        "scico"
    }
}

/// Whether `text` already is one of the protocol's containers.
///
/// Hosts build richer containers than [`ScicoAttach`] itself does — the
/// document buffer adds `cursor`, a selection adds `range` and `digest` — and
/// hand them in as attachment content. Those must pass through untouched.
fn is_container(text: &str) -> bool {
    text.trim_start().starts_with("<scico:v1")
}

/// Return `text` shortened to at most `max` characters, and whether it was cut.
fn truncate(text: &str, max: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= max {
        return (text.to_owned(), false);
    }
    (text.chars().take(max).collect(), true)
}

/// Escape the few characters that would end or corrupt an XML-like attribute.
fn attribute(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(name: &str, content: &str) -> AttachedDocument {
        AttachedDocument {
            name: name.to_owned(),
            content: content.to_owned(),
        }
    }

    #[test]
    fn attachments_and_retrieval_use_the_documented_containers() {
        let merged = ScicoAttach::new().merge(
            "[doc: Kano2025]\nretrieved passage",
            &[
                document("@Kano2025", "PAPER BODY"),
                document("@Kobayashi2011", "OTHER BODY"),
            ],
        );

        assert_eq!(
            merged,
            "<scico:v1:context source=\"creator\">\n\
             <scico:v1:document name=\"@Kano2025\">PAPER BODY</scico:v1:document>\n\
             <scico:v1:document name=\"@Kobayashi2011\">OTHER BODY</scico:v1:document>\n\
             </scico:v1:context>\n\n\
             <scico:v1:context source=\"corpus\">\n\
             [doc: Kano2025]\nretrieved passage\n\
             </scico:v1:context>"
        );
    }

    #[test]
    fn empty_parts_do_not_leave_empty_containers() {
        assert_eq!(ScicoAttach::new().merge("", &[]), "");
        assert_eq!(
            ScicoAttach::new().merge("chunks", &[]),
            "<scico:v1:context source=\"corpus\">\nchunks\n</scico:v1:context>"
        );
        assert_eq!(
            ScicoAttach::new().merge("", &[document("@Key", "BODY")]),
            "<scico:v1:context source=\"creator\">\n\
             <scico:v1:document name=\"@Key\">BODY</scico:v1:document>\n\
             </scico:v1:context>"
        );
    }

    #[test]
    fn prebuilt_containers_pass_through_verbatim() {
        let selection = "<scico:v1:selection name=\"draft.md\" range=\"2..8\" chars=\"6\" digest=\"b3:abc\">\nmarked\n</scico:v1:selection>";
        let merged = ScicoAttach::new().merge(
            "",
            &[document("ignored", selection), document("@Key", "PAPER")],
        );

        assert!(
            merged.contains(selection),
            "the richer container is kept: {merged}"
        );
        assert!(
            !merged.contains("<scico:v1:document name=\"ignored\">"),
            "a container is not wrapped twice: {merged}"
        );
        assert!(merged.contains("<scico:v1:document name=\"@Key\">PAPER</scico:v1:document>"));
    }

    #[test]
    fn attribute_values_are_escaped() {
        let merged = ScicoAttach::new().merge("", &[document("a\"<&", "BODY")]);
        assert!(merged.contains("name=\"a&quot;&lt;&amp;\""));
    }

    #[test]
    fn a_token_budget_keeps_the_whole_context_within_the_window() {
        // 1,024 reserved + 100 context tokens at three characters each.
        let merged = ScicoAttach::with_context_tokens(1124)
            .merge(&"r".repeat(1_000), &[document("@Key", &"a".repeat(1_000))]);

        // 70% of the 300-character budget goes to the paper, the rest to retrieval.
        assert!(
            merged
                .contains("<scico:v1:truncated shown=\"210\" total=\"1000\"/></scico:v1:document>")
        );
        assert!(merged.contains(&format!("source=\"corpus\">\n{}\n", "r".repeat(90))));
        let body = merged
            .split_once("name=\"@Key\">")
            .expect("document body")
            .1
            .split_once("<scico:v1:truncated")
            .expect("truncation marker")
            .0;
        assert_eq!(body.chars().count(), 210);
    }

    #[test]
    fn short_papers_do_not_get_a_marker_and_leave_room_for_retrieval() {
        let merged = ScicoAttach::with_context_tokens(1124)
            .merge(&"r".repeat(1_000), &[document("@Key", "SHORT")]);
        assert!(!merged.contains("<scico:v1:truncated"));
        assert!(merged.contains(">SHORT</scico:v1:document>"));
        // The unused attachment share does not starve retrieval.
        assert!(merged.contains(&"r".repeat(90)));
    }
}
