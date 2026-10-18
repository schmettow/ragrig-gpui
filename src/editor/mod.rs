//! The editor roundtrip contract.
//!
//! `ragrig-gpui` does not own an editor. It owns the protocol an editor and a
//! RAG co-writer share: versioned context containers, guarded write-back, and
//! the traits a host implements for its concrete editor component.
//!
//! A host implements [`EditorSurface`] for its document type and
//! [`RoundtripHost`] for the state that owns the active editor, then hands the
//! agent a [`Roundtrip`]. Swapping the editor backend (zorite, gpuikit, …)
//! leaves the protocol and the agent untouched.

mod host;
mod protocol;

pub use host::{
    EditorSnapshot, EditorSurface, ReplacementOutcome, ReplacementTarget, Roundtrip, RoundtripHost,
    SelectionContext, SelectionSnapshot,
};
pub use protocol::{
    ContextNote, EditorBuffer, MARKER_CONTEXT, MARKER_DOCUMENT, MARKER_LIBRARY, MARKER_REPLACE,
    MARKER_SELECTION, MARKER_TRUNCATED, MARKERS, MAX_CONTEXT_CHARS, PROTOCOL, Replacement,
    RoundtripContext, RoundtripKind, byte_to_char_index, char_range_to_bytes, context_note, digest,
    missing_markers, replace_at, verified_bytes,
};
