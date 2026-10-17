//! Reusable [GPUI](https://gpui.rs) components for Ragrig.
//!
//! `ragrig-gpui` packages the chat, composer, transcript, library and settings
//! surfaces needed to build a RAG front end. The crate deliberately contains no
//! editor: hosting applications such as Sciscraper compose these components
//! into their own layout.
//!
//! # Feature flags
//!
//! - `ragrig` (default): enables the Ragrig controller and its Tokio bridge.
//! - `grobid`: enables GROBID/BibTeX reference ingestion through
//!   [`grobid-bibtex`](https://crates.io/crates/grobid-bibtex).

#![forbid(unsafe_code)]
#![warn(missing_docs)]

mod cancel;

pub use cancel::CancelFlag;

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
