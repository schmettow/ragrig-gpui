//! The editor contract used by the roundtrip.
//!
//! Hosts implement [`EditorSurface`] for their concrete editor component and
//! [`RoundtripHost`] for the state that owns it. [`Roundtrip`] turns those
//! implementations into the document/selection contexts the agent sees and
//! routes guarded replacements back to the editor.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui_kit::{App, SharedString, Window};

use super::protocol::{EditorBuffer, RoundtripContext, char_range_to_bytes};

/// An owned snapshot of one editor document.
///
/// The content is a [`SharedString`], so taking a snapshot for a prompt does
/// not copy the buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorSnapshot {
    /// Stable identity of the document inside the host.
    pub id: u64,
    /// Display name (`draft.md`, `Untitled-1`).
    pub name: SharedString,
    /// File backing the document, if any.
    pub path: Option<PathBuf>,
    /// Caret position (line, column, 0-based).
    pub cursor: (usize, usize),
    /// The document source.
    pub content: SharedString,
}

impl EditorSnapshot {
    /// The borrowed view the protocol's containers are built from.
    #[must_use]
    pub fn buffer(&self) -> EditorBuffer<'_> {
        EditorBuffer {
            name: self.name.as_ref(),
            path: self.path.as_deref(),
            cursor: self.cursor,
            content: self.content.as_ref(),
        }
    }
}

/// A marked passage in one editor document.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionSnapshot {
    /// The document the passage belongs to.
    pub document: EditorSnapshot,
    /// The passage, char-indexed and document-relative.
    pub range: Range<usize>,
}

/// The minimal editor surface the roundtrip needs.
///
/// Implement this for the host's document/editor adapter. The concrete editor
/// component (zorite, gpuikit, …) stays behind this trait, so the agent never
/// names a backend.
pub trait EditorSurface {
    /// Stable identity of the document inside the host.
    fn id(&self) -> u64;
    /// Display name shown in tabs and context containers.
    fn name(&self) -> SharedString;
    /// File backing the document, if any.
    fn path(&self) -> Option<&Path>;
    /// The current source, as a cheap shared handle.
    fn text(&self, cx: &App) -> SharedString;
    /// Caret position (line, column, 0-based).
    fn cursor(&self, cx: &App) -> (usize, usize);
    /// The marked passage as a char range, or `None` when nothing is marked.
    fn selection(&self, window: &mut Window, cx: &mut App) -> Option<Range<usize>>;
    /// Replace a byte range as one undo step.
    fn replace(&self, range: Range<usize>, text: &str, window: &mut Window, cx: &mut App);
    /// Focus this editor.
    fn focus(&self, window: &mut Window, cx: &mut App);

    /// The caret as a char index, whether or not a selection is marked.
    ///
    /// The library uses this to insert a citation at the caret. Hosts whose
    /// editor exposes a native caret can override the default, which computes
    /// it from the snapshot's line and column.
    fn caret(&self, cx: &App) -> usize {
        let snapshot = self.snapshot(cx);
        char_index_for_line_column(&snapshot.content, snapshot.cursor)
    }

    /// Insert `text` at the selection, or at the caret when nothing is marked.
    ///
    /// The default is backend-independent: it converts the char-indexed
    /// selection to bytes and calls [`Self::replace`], so the insertion is one
    /// undo step in every editor that implements the contract. This is the
    /// Library → Editor half of the roundtrip.
    fn insert_text(&self, text: &str, window: &mut Window, cx: &mut App) {
        if text.is_empty() {
            return;
        }
        let content = self.text(cx);
        let range = self.selection(window, cx).unwrap_or_else(|| {
            let caret = self.caret(cx);
            caret..caret
        });
        self.replace(char_range_to_bytes(&content, range), text, window, cx);
    }

    /// An owned snapshot suitable for prompt building.
    fn snapshot(&self, cx: &App) -> EditorSnapshot {
        EditorSnapshot {
            id: self.id(),
            name: self.name(),
            path: self.path().map(Path::to_path_buf),
            cursor: self.cursor(cx),
            content: self.text(cx),
        }
    }
}

/// The char index of a 0-based `(line, column)` in `content`.
///
/// The column counts chars, matching [`EditorSnapshot::cursor`]. A position
/// past the end clamps to the last char.
#[must_use]
pub fn char_index_for_line_column(content: &str, cursor: (usize, usize)) -> usize {
    let mut line = 0usize;
    let mut column = 0usize;
    for (byte, character) in content.char_indices() {
        if line == cursor.0 && column == cursor.1 {
            return content[..byte].chars().count();
        }
        if character == '\n' {
            line += 1;
            column = 0;
        } else {
            column += 1;
        }
    }
    content.chars().count()
}

/// The host side of the roundtrip: the active editor and its write-back.
pub trait RoundtripHost {
    /// The active document, if any.
    fn active_document(&self, cx: &App) -> Option<EditorSnapshot>;
    /// The active editor's marked passage, if any.
    fn selection(&self, window: &mut Window, cx: &mut App) -> Option<SelectionSnapshot>;
    /// Insert text at the active selection or caret.
    ///
    /// This is the Library → Editor direction: the host owns the editor, so it
    /// performs the insertion through its [`EditorSurface`] implementation.
    fn insert(&self, _text: &str, _window: &mut Window, _cx: &mut App) -> bool {
        false
    }
    /// Write a replacement back, guarded by the selection digest.
    fn apply_replacement(
        &self,
        target: ReplacementTarget,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> ReplacementOutcome;
}

/// Where a guarded replacement should land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReplacementTarget {
    /// The document identity captured with the selection.
    pub document_id: u64,
    /// The char range captured with the selection.
    pub range: Range<usize>,
    /// The digest of the marked passage at capture time.
    pub digest: String,
}

/// The result of trying to write an agent replacement into an editor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReplacementOutcome {
    /// The passage still matched its digest and was replaced.
    Applied {
        /// A transcript-ready summary.
        message: String,
    },
    /// The passage changed (or the document vanished); offer the text instead.
    Offered {
        /// The replacement text.
        text: String,
        /// Why it was not applied.
        reason: String,
    },
    /// There was nothing to write.
    Empty,
}

/// A captured selection together with the document it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectionContext {
    /// The document identity, used as the write-back target.
    pub document_id: u64,
    /// The protocol container for the passage.
    pub context: RoundtripContext,
}

/// The roundtrip façade the agent holds.
///
/// The host implements [`RoundtripHost`] once; this type builds document and
/// selection contexts and applies replacements without knowing the editor.
#[derive(Clone)]
pub struct Roundtrip {
    host: Rc<dyn RoundtripHost>,
}

impl Roundtrip {
    /// Wrap a host implementation.
    pub fn new(host: Rc<dyn RoundtripHost>) -> Self {
        Self { host }
    }

    /// The active document, if any.
    #[must_use]
    pub fn active_document(&self, cx: &App) -> Option<EditorSnapshot> {
        self.host.active_document(cx)
    }

    /// The active document as a prompt container.
    #[must_use]
    pub fn document_context(&self, cx: &App, limit: usize) -> Option<RoundtripContext> {
        let document = self.host.active_document(cx)?;
        RoundtripContext::document_with_limit(document.buffer(), limit)
    }

    /// The active selection as a prompt container.
    #[must_use]
    pub fn selection_context(
        &self,
        window: &mut Window,
        cx: &mut App,
        limit: usize,
    ) -> Option<SelectionContext> {
        let selection = self.host.selection(window, cx)?;
        let context = RoundtripContext::selection_with_limit(
            selection.document.buffer(),
            selection.range,
            limit,
        )?;
        Some(SelectionContext {
            document_id: selection.document.id,
            context,
        })
    }

    /// Apply a guarded replacement through the host.
    pub fn apply(
        &self,
        target: ReplacementTarget,
        text: &str,
        window: &mut Window,
        cx: &mut App,
    ) -> ReplacementOutcome {
        self.host.apply_replacement(target, text, window, cx)
    }

    /// Insert text at the active selection or caret.
    ///
    /// Returns `false` when no document is open.
    pub fn insert(&self, text: &str, window: &mut Window, cx: &mut App) -> bool {
        self.host.insert(text, window, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct MockEditor {
        content: RefCell<String>,
        caret: RefCell<usize>,
        selection: RefCell<Option<Range<usize>>>,
    }

    impl MockEditor {
        fn new(content: &str, caret: usize) -> Self {
            Self {
                content: RefCell::new(content.to_owned()),
                caret: RefCell::new(caret),
                selection: RefCell::new(None),
            }
        }

        fn selected(mut self, range: Range<usize>) -> Self {
            self.selection = RefCell::new(Some(range));
            self
        }

        fn value(&self) -> String {
            self.content.borrow().clone()
        }
    }

    impl EditorSurface for MockEditor {
        fn id(&self) -> u64 {
            1
        }

        fn name(&self) -> SharedString {
            "mock.md".into()
        }

        fn path(&self) -> Option<&Path> {
            None
        }

        fn text(&self, _: &App) -> SharedString {
            self.content.borrow().clone().into()
        }

        fn cursor(&self, _: &App) -> (usize, usize) {
            let content = self.content.borrow();
            let caret = (*self.caret.borrow()).min(content.chars().count());
            let before: String = content.chars().take(caret).collect();
            (
                before.matches('\n').count(),
                before.rsplit('\n').next().unwrap_or("").chars().count(),
            )
        }

        fn selection(&self, _: &mut Window, _: &mut App) -> Option<Range<usize>> {
            self.selection.borrow().clone()
        }

        fn replace(&self, range: Range<usize>, text: &str, _: &mut Window, _: &mut App) {
            let content = self.content.borrow().clone();
            let mut next = content[..range.start].to_owned();
            next.push_str(text);
            next.push_str(&content[range.end..]);
            *self.caret.borrow_mut() =
                content[..range.start].chars().count() + text.chars().count();
            *self.content.borrow_mut() = next;
            *self.selection.borrow_mut() = None;
        }

        fn focus(&self, _: &mut Window, _: &mut App) {}
    }

    struct Root;

    impl gpui_kit::Render for Root {
        fn render(
            &mut self,
            _: &mut Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            gpui_kit::div()
        }
    }

    #[gpui_kit::test]
    fn the_default_insert_lands_at_the_caret(cx: &mut gpui_kit::TestAppContext) {
        let (_root, cx) = cx.add_window_view(|_, _| Root);
        let editor = MockEditor::new("hello world", 5);
        cx.update(|window, cx| editor.insert_text(" brave", window, cx));
        assert_eq!(editor.value(), "hello brave world");
    }

    #[gpui_kit::test]
    fn the_default_insert_replaces_the_selection(cx: &mut gpui_kit::TestAppContext) {
        let (_root, cx) = cx.add_window_view(|_, _| Root);
        let editor = MockEditor::new("hello world", 0).selected(0..5);
        cx.update(|window, cx| editor.insert_text("hi", window, cx));
        assert_eq!(editor.value(), "hi world");
    }

    #[test]
    fn line_columns_convert_to_char_indices() {
        assert_eq!(char_index_for_line_column("abc", (0, 0)), 0);
        assert_eq!(char_index_for_line_column("abc", (0, 3)), 3);
        assert_eq!(char_index_for_line_column("ab\ncd", (1, 1)), 4);
        assert_eq!(char_index_for_line_column("ab\ncd", (1, 99)), 5);
        assert_eq!(char_index_for_line_column("ab\ncd", (9, 0)), 5);
    }
}
