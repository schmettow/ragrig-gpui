//! The editor-agnostic container protocol of the roundtrip.
//!
//! Everything the agent is shown from the editor is wrapped in a versioned,
//! self-describing container, so the chat prompt can tell the author's
//! own text apart from retrieved literature:
//!
//! ```text
//! <scico:v1:context source="creator">
//! <scico:v1:document name="paper.md" path="docs/paper.md" cursor="42:7" chars="12345">
//! …source…
//! </scico:v1:document>
//! <scico:v1:selection name="paper.md" range="120..480" chars="360" digest="b3:…">
//! …the marked passage…
//! </scico:v1:selection>
//! </scico:v1:context>
//! ```
//!
//! The result half is [`Replacement::extract`]: the agent answers with a
//! fenced `scico:v1:replace` block, the host lifts it out of the chat and
//! writes it back only when [`digest`] still matches the marked passage.

use std::ops::Range;
use std::path::{Path, PathBuf};

/// Protocol version prefix shared by every marker.
pub const PROTOCOL: &str = "scico:v1";
/// Container for one block of context.
pub const MARKER_CONTEXT: &str = "scico:v1:context";
/// Container for one document inside a context block.
pub const MARKER_DOCUMENT: &str = "scico:v1:document";
/// Container for the author's bibliography.
pub const MARKER_LIBRARY: &str = "scico:v1:library";
/// Container for a passage the author marked for a question.
pub const MARKER_SELECTION: &str = "scico:v1:selection";
/// Marks where a truncated context was cut.
pub const MARKER_TRUNCATED: &str = "scico:v1:truncated";
/// Info string of the result-side replacement block.
pub const MARKER_REPLACE: &str = "scico:v1:replace";

/// One attached context carries at most this many characters.
pub const MAX_CONTEXT_CHARS: usize = 100_000;

/// Every marker the protocol defines, in prompt order.
pub const MARKERS: &[&str] = &[
    MARKER_CONTEXT,
    MARKER_DOCUMENT,
    MARKER_LIBRARY,
    MARKER_SELECTION,
    MARKER_TRUNCATED,
    MARKER_REPLACE,
];

/// The markers `prompt` does not mention.
#[must_use]
pub fn missing_markers(prompt: &str) -> Vec<&'static str> {
    MARKERS
        .iter()
        .copied()
        .filter(|marker| !prompt.contains(marker))
        .collect()
}

/// The editor facts a [`RoundtripContext`] is built from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EditorBuffer<'a> {
    /// Display name of the document (`draft.md`, `Untitled-1`).
    pub name: &'a str,
    /// Its path, when it is backed by a file.
    pub path: Option<&'a Path>,
    /// Caret position (line, column, 0-based).
    pub cursor: (usize, usize),
    /// The document source.
    pub content: &'a str,
}

impl EditorBuffer<'_> {
    /// Whether the buffer carries text worth attaching.
    #[must_use]
    pub fn has_text(&self) -> bool {
        !self.content.trim().is_empty()
    }
}

/// Which region of the editor a context came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RoundtripKind {
    /// The active document.
    Document,
    /// The author's bibliography.
    Library,
    /// A passage the author marked.
    Selection,
}

impl RoundtripKind {
    /// The container marker for this kind.
    #[must_use]
    pub fn marker(self) -> &'static str {
        match self {
            Self::Document => MARKER_DOCUMENT,
            Self::Library => MARKER_LIBRARY,
            Self::Selection => MARKER_SELECTION,
        }
    }
}

/// What the transcript needs to note an attached context.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ContextNote {
    /// Which region was attached.
    pub kind: RoundtripKind,
    /// Display name of the document the region came from.
    pub name: String,
    /// Characters in the full text (before truncation).
    pub chars: usize,
    /// Whether the model only saw an excerpt.
    pub truncated: bool,
    /// Characters the model actually saw.
    pub shown: usize,
}

/// One region of the editor attached to a query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RoundtripContext {
    kind: RoundtripKind,
    name: String,
    path: Option<PathBuf>,
    cursor: Option<(usize, usize)>,
    range: Option<Range<usize>>,
    digest: String,
    chars: usize,
    truncated: bool,
    limit: usize,
    text: String,
}

impl RoundtripContext {
    /// The active document, or `None` when there is nothing worth attaching.
    #[must_use]
    pub fn document(buffer: EditorBuffer<'_>) -> Option<Self> {
        Self::document_with_limit(buffer, MAX_CONTEXT_CHARS)
    }

    /// The active document, shortened to at most `limit` characters when the
    /// model window is smaller than the protocol's hard cap.
    #[must_use]
    pub fn document_with_limit(buffer: EditorBuffer<'_>, limit: usize) -> Option<Self> {
        if !buffer.has_text() {
            return None;
        }
        let limit = limit.clamp(1, MAX_CONTEXT_CHARS);
        let (text, chars, truncated) = excerpt(buffer.content, limit);
        Some(Self {
            kind: RoundtripKind::Document,
            name: buffer.name.to_string(),
            path: buffer.path.map(Path::to_path_buf),
            cursor: Some(buffer.cursor),
            range: None,
            digest: digest(buffer.content),
            chars,
            truncated,
            limit,
            text,
        })
    }

    /// The author's bibliography, attached as a whole.
    #[must_use]
    pub fn library(path: &Path, content: &str) -> Option<Self> {
        Self::library_with_limit(path, content, MAX_CONTEXT_CHARS)
    }

    /// The bibliography, shortened to at most `limit` characters.
    #[must_use]
    pub fn library_with_limit(path: &Path, content: &str, limit: usize) -> Option<Self> {
        if content.trim().is_empty() {
            return None;
        }
        let limit = limit.clamp(1, MAX_CONTEXT_CHARS);
        let (text, chars, truncated) = excerpt(content, limit);
        Some(Self {
            kind: RoundtripKind::Library,
            name: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string()),
            path: Some(path.to_path_buf()),
            cursor: None,
            range: None,
            digest: digest(content),
            chars,
            truncated,
            limit,
            text,
        })
    }

    /// A passage the author marked, or `None` when the marked text is blank.
    ///
    /// `range` is char-indexed and document-relative, matching
    /// [`replace_at`]. The digest covers the full marked text, so an answer
    /// stays checkable even when the model only saw a truncated excerpt.
    #[must_use]
    pub fn selection(buffer: EditorBuffer<'_>, range: Range<usize>) -> Option<Self> {
        Self::selection_with_limit(buffer, range, MAX_CONTEXT_CHARS)
    }

    /// A marked passage, shortened to at most `limit` characters.
    #[must_use]
    pub fn selection_with_limit(
        buffer: EditorBuffer<'_>,
        range: Range<usize>,
        limit: usize,
    ) -> Option<Self> {
        debug_assert!(range.start <= range.end, "range must be ordered: {range:?}");
        let total = buffer.content.chars().count();
        let start = range.start.min(total);
        let end = range.end.min(total).max(start);
        let marked: String = buffer
            .content
            .chars()
            .skip(start)
            .take(end - start)
            .collect();
        if marked.trim().is_empty() {
            return None;
        }
        let limit = limit.clamp(1, MAX_CONTEXT_CHARS);
        let (text, chars, truncated) = excerpt(&marked, limit);
        Some(Self {
            kind: RoundtripKind::Selection,
            name: buffer.name.to_string(),
            path: buffer.path.map(Path::to_path_buf),
            cursor: None,
            range: Some(start..end),
            digest: digest(&marked),
            chars,
            truncated,
            limit,
            text,
        })
    }

    /// The raw passage text, without the protocol container.
    ///
    /// Enriched retrieval uses this to embed a marked passage together with
    /// the question.
    #[must_use]
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The container as the model sees it.
    #[must_use]
    pub fn container(&self) -> String {
        let marker = self.kind.marker();
        let mut out = format!(
            "<{marker}{}>\n{}\n",
            self.attributes(),
            self.text.trim_end()
        );
        if self.truncated {
            out.push_str(&format!(
                "<{MARKER_TRUNCATED} shown=\"{}\" total=\"{}\"/>\n",
                self.text.chars().count(),
                self.chars
            ));
        }
        out.push_str(&format!("</{marker}>"));
        out
    }

    /// A pending attachment whose content is this container.
    ///
    /// The worker's `ScicoAttach` passes pre-built containers through, so
    /// the attributes survive unchanged.
    #[must_use]
    pub fn attachment_label(&self) -> String {
        match self.kind {
            RoundtripKind::Document => self.name.clone(),
            RoundtripKind::Library => self.name.clone(),
            RoundtripKind::Selection => format!("selection {}", self.name),
        }
    }

    /// The facts the transcript note is built from.
    #[must_use]
    pub fn note(&self) -> ContextNote {
        ContextNote {
            kind: self.kind,
            name: self.name.clone(),
            chars: self.chars,
            truncated: self.truncated,
            shown: self.text.chars().count(),
        }
    }

    /// Characters in the full text (before truncation).
    #[must_use]
    pub fn chars(&self) -> usize {
        self.chars
    }

    /// A copy shortened to at most `limit` characters.
    ///
    /// Used when a small model window cannot fit the protocol's hard cap; the
    /// full length and digest stay attached to the shortened container.
    #[must_use]
    pub fn shortened(&self, limit: usize) -> Self {
        let limit = limit.clamp(1, MAX_CONTEXT_CHARS);
        let (text, _, _) = excerpt(&self.text, limit);
        let mut shortened = self.clone();
        shortened.text = text;
        shortened.truncated = self.chars > limit;
        shortened.limit = limit;
        shortened
    }

    /// The char range of the marked passage, for selections only.
    #[must_use]
    pub fn range(&self) -> Option<Range<usize>> {
        self.range.clone()
    }

    /// Identity of the full text, for write-back guards.
    #[must_use]
    pub fn digest(&self) -> &str {
        &self.digest
    }

    /// Which part of the editor this context came from.
    #[must_use]
    pub fn kind(&self) -> RoundtripKind {
        self.kind
    }

    fn attributes(&self) -> String {
        let mut out = format!(" name=\"{}\"", attribute(&self.name));
        if let Some(path) = &self.path {
            out.push_str(&format!(
                " path=\"{}\"",
                attribute(&path.display().to_string())
            ));
        }
        if let Some((line, column)) = self.cursor {
            out.push_str(&format!(" cursor=\"{line}:{column}\""));
        }
        if let Some(range) = &self.range {
            out.push_str(&format!(" range=\"{}..{}\"", range.start, range.end));
        }
        out.push_str(&format!(" chars=\"{}\"", self.chars));
        if self.kind == RoundtripKind::Selection {
            out.push_str(&format!(" digest=\"{}\"", self.digest));
        }
        out
    }
}

/// The note a transcript shows for an attached context.
#[must_use]
pub fn context_note(note: &ContextNote) -> String {
    let label = match note.kind {
        RoundtripKind::Document => "Document attached",
        RoundtripKind::Library => "Library attached",
        RoundtripKind::Selection => "Marked region attached",
    };
    if note.truncated {
        format!(
            "{label}: {} (first {} of {} chars)",
            note.name, note.shown, note.chars
        )
    } else {
        format!("{label}: {} ({} chars)", note.name, note.chars)
    }
}

/// Cut `text` to [`MAX_CONTEXT_CHARS`], with its full length and whether the
/// cut happened — char-counted, so multi-byte text never splits a codepoint.
fn excerpt(text: &str, limit: usize) -> (String, usize, bool) {
    let mut out = String::new();
    let mut chars = 0usize;
    for character in text.chars() {
        chars += 1;
        if chars <= limit {
            out.push(character);
        }
    }
    (out, chars, chars > limit)
}

/// A short, stable identity for one version of a text.
#[must_use]
pub fn digest(text: &str) -> String {
    let hash = blake3::hash(text.as_bytes()).to_hex();
    format!("b3:{}", &hash.as_str()[..16])
}

/// Replace the character range `range` of `content` with `replacement`.
///
/// Char-indexed, so multi-byte text and emoji never split a codepoint. A range
/// past the end clamps to the length.
#[must_use]
pub fn replace_at(content: &str, range: Range<usize>, replacement: &str) -> String {
    debug_assert!(range.start <= range.end, "range must be ordered: {range:?}");
    let mut out = String::with_capacity(content.len() + replacement.len());
    out.extend(content.chars().take(range.start));
    out.push_str(replacement);
    out.extend(content.chars().skip(range.end));
    out
}

/// The byte range of a char range, clamped to `text`.
#[must_use]
pub fn char_range_to_bytes(text: &str, range: Range<usize>) -> Range<usize> {
    let start = text
        .char_indices()
        .nth(range.start)
        .map_or(text.len(), |(offset, _)| offset);
    let end = text
        .char_indices()
        .nth(range.end)
        .map_or(text.len(), |(offset, _)| offset);
    start..end.max(start)
}

/// The byte range of a marked passage, but only when its digest still
/// matches `guard`.
///
/// This is the write-back guard: the agent may have started from a version
/// the author has since edited, and a replacement must never land in text it
/// was not written for.
#[must_use]
pub fn verified_bytes(content: &str, range: Range<usize>, guard: &str) -> Option<Range<usize>> {
    let total = content.chars().count();
    let start = range.start.min(total);
    let end = range.end.min(total).max(start);
    let marked: String = content.chars().skip(start).take(end - start).collect();
    (digest(&marked) == guard).then(|| char_range_to_bytes(content, start..end))
}

/// The char index of a byte offset.
#[must_use]
pub fn byte_to_char_index(text: &str, byte: usize) -> usize {
    text[..byte.min(text.len())].chars().count()
}

/// The result half of the protocol: one replacement an answer asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Replacement {
    /// The passage the answer names in its `target="a..b"` attribute.
    ///
    /// Informational: the host applies at the range the author actually
    /// marked, never at what the model says.
    pub target: Option<Range<usize>>,
    /// The finished text for the passage.
    pub body: String,
}

impl Replacement {
    /// Lift every replacement block out of `answer`.
    ///
    /// Returns the prose with the blocks removed and the blocks in document
    /// order. Only fenced blocks whose info string starts with
    /// [`MARKER_REPLACE`] are lifted; every other code block stays in the
    /// prose.
    #[must_use]
    pub fn extract(answer: &str) -> Option<(String, Vec<Self>)> {
        let arena = comrak::Arena::new();
        let root = comrak::parse_document(&arena, answer, &comrak::Options::default());
        let mut blocks = Vec::new();
        let mut fences: Vec<std::ops::RangeInclusive<usize>> = Vec::new();
        for node in root.descendants() {
            let data = node.data.borrow();
            let comrak::nodes::NodeValue::CodeBlock(code) = &data.value else {
                continue;
            };
            if info_marker(&code.info) != Some(MARKER_REPLACE) {
                continue;
            }
            blocks.push(Self {
                target: target_attribute(&code.info),
                body: code
                    .literal
                    .strip_suffix('\n')
                    .unwrap_or(&code.literal)
                    .to_string(),
            });
            fences.push(data.sourcepos.start.line..=data.sourcepos.end.line);
        }
        if blocks.is_empty() {
            return None;
        }
        let mut prose = String::new();
        for (index, line) in answer.lines().enumerate() {
            if fences.iter().any(|fence| fence.contains(&(index + 1))) {
                continue;
            }
            if line.trim().is_empty() && prose.ends_with("\n\n") {
                continue;
            }
            prose.push_str(line);
            prose.push('\n');
        }
        Some((prose.trim().to_string(), blocks))
    }
}

/// The first word of a code block's info string.
fn info_marker(info: &str) -> Option<&str> {
    info.split_whitespace().next()
}

/// The `target="a..b"` attribute of a replacement info string.
fn target_attribute(info: &str) -> Option<Range<usize>> {
    let value = info
        .split_once("target=")
        .map(|(_, rest)| rest.trim_start())?
        .strip_prefix('"')?;
    let value = value.split('"').next()?;
    let (start, end) = value.split_once("..")?;
    Some(start.trim().parse().ok()?..end.trim().parse().ok()?)
}

/// Strip everything that would break a quoted XML-like attribute.
fn attribute(value: &str) -> String {
    value
        .chars()
        .map(|c| match c {
            '"' | '<' | '>' | '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffer<'a>(name: &'a str, path: Option<&'a Path>, content: &'a str) -> EditorBuffer<'a> {
        EditorBuffer {
            name,
            path,
            cursor: (2, 3),
            content,
        }
    }

    #[test]
    fn a_document_container_carries_name_path_cursor_and_length() {
        let path = Path::new("/tmp/draft.md");
        let context = RoundtripContext::document(buffer("draft.md", Some(path), "hello world"))
            .expect("non-empty document");
        let container = context.container();

        assert!(container.starts_with("<scico:v1:document"));
        assert!(container.contains("name=\"draft.md\""));
        assert!(container.contains("path=\"/tmp/draft.md\""));
        assert!(container.contains("cursor=\"2:3\""));
        assert!(container.contains("chars=\"11\""));
        assert!(!container.contains("digest="));
        assert!(container.ends_with("</scico:v1:document>"));
    }

    #[test]
    fn a_blank_document_attaches_nothing() {
        assert!(RoundtripContext::document(buffer("draft.md", None, " \n ")).is_none());
    }

    #[test]
    fn a_selection_is_char_indexed_and_multibyte_safe() {
        let text = "aé😀z";
        let context =
            RoundtripContext::selection(buffer("draft.md", None, text), 1..3).expect("selection");
        assert_eq!(context.range(), Some(1..3));
        assert!(context.container().contains("\né😀\n"));
        assert!(context.container().contains("range=\"1..3\""));
        assert!(context.container().contains(context.digest()));
    }

    #[test]
    fn a_blank_selection_attaches_nothing() {
        assert!(RoundtripContext::selection(buffer("draft.md", None, "abc"), 1..1).is_none());
    }

    #[test]
    fn a_library_container_has_no_cursor() {
        let context = RoundtripContext::library(Path::new("/tmp/library.bib"), "@article{A}")
            .expect("library");
        let container = context.container();
        assert!(container.starts_with("<scico:v1:library"));
        assert!(!container.contains("cursor="));
        assert!(container.contains("name=\"library.bib\""));
    }

    #[test]
    fn an_oversized_document_is_truncated_with_a_marker() {
        let text = "a".repeat(MAX_CONTEXT_CHARS + 10);
        let context =
            RoundtripContext::document(buffer("draft.md", None, &text)).expect("document");
        assert!(context.truncated);
        assert_eq!(context.chars(), MAX_CONTEXT_CHARS + 10);
        assert!(context.container().contains(&format!(
            "<{MARKER_TRUNCATED} shown=\"{MAX_CONTEXT_CHARS}\""
        )));
    }

    #[test]
    fn attribute_values_cannot_break_the_tag() {
        let context =
            RoundtripContext::document(buffer("a\"<b>\nc", None, "text")).expect("document");
        assert!(context.container().contains("name=\"a b c\""));
    }

    #[test]
    fn replace_at_is_char_indexed_and_clamps() {
        assert_eq!(replace_at("aé😀z", 1..3, "X"), "aXz");
        assert_eq!(replace_at("abc", 2..99, "Z"), "abZ");
        assert_eq!(replace_at("abc", 1..1, "Z"), "aZbc");
    }

    #[test]
    fn char_and_byte_ranges_round_trip() {
        let text = "aé😀z";
        assert_eq!(char_range_to_bytes(text, 1..3), 1..7);
        assert_eq!(byte_to_char_index(text, 7), 3);
        assert_eq!(char_range_to_bytes(text, 2..99), 3..8);
    }

    #[test]
    fn a_verified_range_requires_the_marked_version() {
        let text = "aé😀z";
        let guard = digest("é😀");
        assert_eq!(verified_bytes(text, 1..3, &guard), Some(1..7));
        assert_eq!(verified_bytes(text, 1..3, &digest("changed")), None);
        assert_eq!(verified_bytes(text, 2..99, &digest("😀z")), Some(3..8));
    }

    #[test]
    fn digests_are_stable_and_distinguish_versions() {
        assert_eq!(digest("abc"), digest("abc"));
        assert_ne!(digest("abc"), digest("abd"));
        assert!(digest("abc").starts_with("b3:"));
        assert_eq!(digest("abc").len(), 19);
    }

    #[test]
    fn a_replacement_block_is_lifted_out_of_the_answer() {
        let answer =
            "Here is the shorter version.\n\n```scico:v1:replace target=\"4..9\"\nnew text\n```\n";
        let (prose, blocks) = Replacement::extract(answer).expect("a replacement block");
        assert_eq!(prose, "Here is the shorter version.");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].target, Some(4..9));
        assert_eq!(blocks[0].body, "new text");
    }

    #[test]
    fn a_block_before_the_prose_is_lifted_too() {
        let answer = "```scico:v1:replace\nbody\n```\n\nProse follows.";
        let (prose, blocks) = Replacement::extract(answer).expect("a replacement block");
        assert_eq!(prose, "Prose follows.");
        assert_eq!(blocks[0].body, "body");
        assert_eq!(blocks[0].target, None);
    }

    #[test]
    fn only_our_blocks_are_lifted() {
        let answer = "```rust\nfn main() {}\n```";
        assert_eq!(Replacement::extract(answer), None);
    }

    #[test]
    fn a_plain_answer_carries_no_block() {
        assert_eq!(Replacement::extract("just prose"), None);
    }

    #[test]
    fn an_empty_block_is_still_lifted() {
        let (prose, blocks) =
            Replacement::extract("Prose.\n\n```scico:v1:replace\n```").expect("block");
        assert_eq!(prose, "Prose.");
        assert_eq!(blocks[0].body, "");
    }

    #[test]
    fn several_blocks_come_back_in_order() {
        let answer = "```scico:v1:replace target=\"0..1\"\none\n```\n\
                      ```scico:v1:replace target=\"1..2\"\ntwo\n```";
        let (_, blocks) = Replacement::extract(answer).expect("blocks");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0].body, "one");
        assert_eq!(blocks[1].body, "two");
    }

    #[test]
    fn a_garbled_target_is_not_a_target() {
        let (_, blocks) =
            Replacement::extract("```scico:v1:replace target=\"x..y\"\nbody\n```").expect("block");
        assert_eq!(blocks[0].target, None);
    }

    #[test]
    fn a_custom_limit_shortens_a_context_and_keeps_the_full_length() {
        let text = "a".repeat(500);
        let context = RoundtripContext::document_with_limit(buffer("draft.md", None, &text), 120)
            .expect("document");
        assert!(context.truncated);
        assert_eq!(context.note().shown, 120);
        assert_eq!(context.note().chars, 500);
        assert!(context.container().contains("shown=\"120\" total=\"500\""));
    }

    #[test]
    fn shortened_keeps_the_digest_and_respects_the_new_limit() {
        let selected =
            RoundtripContext::selection(buffer("draft.md", None, &"x".repeat(400)), 0..400)
                .expect("selection");
        let short = selected.shortened(90);
        assert_eq!(short.digest(), selected.digest());
        assert!(short.truncated);
        assert_eq!(short.note().shown, 90);
        assert!(short.container().contains("chars=\"400\""));
    }

    #[test]
    fn notes_name_the_region_and_its_size() {
        let context =
            RoundtripContext::document(buffer("draft.md", None, "hello")).expect("document");
        assert_eq!(
            context_note(&context.note()),
            "Document attached: draft.md (5 chars)"
        );
        assert_eq!(context.attachment_label(), "draft.md");
    }
}
