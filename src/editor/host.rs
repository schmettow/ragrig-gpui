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

use super::protocol::{EditorBuffer, RoundtripContext};

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

/// The host side of the roundtrip: the active editor and its write-back.
pub trait RoundtripHost {
    /// The active document, if any.
    fn active_document(&self, cx: &App) -> Option<EditorSnapshot>;
    /// The active editor's marked passage, if any.
    fn selection(&self, window: &mut Window, cx: &mut App) -> Option<SelectionSnapshot>;
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
}
