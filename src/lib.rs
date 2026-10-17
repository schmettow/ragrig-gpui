//! Reusable [GPUI](https://gpui.rs) components for Ragrig.
//!
//! `ragrig-gpui` packages the chat, composer, transcript, attachment, session,
//! agent-status and settings surfaces needed to build a RAG front end. The
//! crate contains no editor: hosting applications such as Sciscraper compose
//! these components into their own layout.
//!
//! # State model
//!
//! The reducers in [`ChatState`] are plain data. [`ChatView`] wraps one in an
//! entity, keeps one `TextViewState` per message for incremental Markdown, and
//! renders it with `MessageScroller`. [`Composer`] owns its input and emits
//! [`ComposerEvent`]s; it does not know about a controller.
//!
//! # Feature flags
//!
//! - `ragrig` (default): enables the Ragrig controller, full settings page,
//!   and the composite [`RagChat`] surface. The worker uses a dedicated Tokio
//!   runtime thread.
//! - `grobid`: enables GROBID/BibTeX reference ingestion through
//!   [`grobid-bibtex`](https://crates.io/crates/grobid-bibtex), the reference
//!   rows, and their hover copy actions. It implies `ragrig`.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod agent_bar;
mod attachments;
mod cancel;
mod chat_view;
mod composer;
mod markdown;
mod model;
#[cfg(feature = "ragrig")]
mod rag;
#[cfg(feature = "grobid")]
mod references;
mod session_picker;
mod settings;
mod side_panel;

pub mod prelude;

pub use agent_bar::{AgentBar, AgentBarEvent};
pub use attachments::{AttachmentEvent, AttachmentRows};
pub use cancel::CancelFlag;
pub use chat_view::{ChatView, ChatViewEvent, EmptyState};
pub use composer::{Composer, ComposerEvent};
pub use markdown::stabilize_markdown;
pub use model::*;
#[cfg(feature = "ragrig")]
pub use rag::*;
#[cfg(feature = "grobid")]
pub use references::{
    ReferenceCopy, ReferenceEntry, ReferenceOptions, ReferenceProgress, ReferenceReport,
    ReferenceRow, ReferenceStage,
};
pub use session_picker::{SessionPicker, SessionPickerEvent};
pub use settings::{chat_settings, chat_settings_page};
pub use side_panel::SidePanel;

use gpui_kit::App;

/// Initializes the GPUI kit and Ragrig UI state.
///
/// Call once after constructing the application and before opening windows:
///
/// ```no_run
/// use gpui_kit::application;
///
/// application().run(|cx| {
///     ragrig_gpui::init(cx);
///     // open your window here
/// });
/// ```
pub fn init(cx: &mut App) {
    gpui_kit::init(cx);
}
