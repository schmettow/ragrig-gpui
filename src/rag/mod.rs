//! Ragrig controller, settings, and the composite chat surface.
//!
//! The module is available with the default `ragrig` feature. It contains:
//!
//! - [`RagController`] — owns the Tokio worker and mirrors its state,
//! - [`RagChat`] — transcript, attachments, and composer wired to the
//!   controller,
//! - [`RagSettings`] with [`rag_settings_page`] — full Ragrig configuration
//!   and workspace profiles.
//!
//! Ragrig runs on a dedicated Tokio runtime thread. Do not drive its futures
//! from GPUI's executor.

mod actor;
mod attach;
mod chat;
mod config;
mod controller;
#[cfg(feature = "managed")]
mod onboarding;
mod protocol;
mod settings_page;

pub use attach::ScicoAttach;
pub use chat::{RagChat, RagChatEvent};
pub use config::{
    ChatProvider, DEFAULT_GROBID_CONTAINER, DEFAULT_GROBID_IMAGE, DEFAULT_GROBID_URL,
    DEFAULT_LLAMACPP_CONTAINER, DEFAULT_LLAMACPP_IMAGE, DEFAULT_OLLAMA_CONTAINER,
    DEFAULT_OLLAMA_IMAGE, EmbedProvider, GrobidSource, MemoryChoice, PdfBackend, RagChanges,
    RagField, RagSection, RagSettings, RagSetup, ServiceSource,
};
pub use controller::RagController;
#[cfg(feature = "managed")]
pub use onboarding::{
    OnboardingAction, OnboardingActions, OnboardingStep, RagOnboarding, SETUP_MARKER,
    TokenSuggestion, mark_workspace_setup_done, onboarding_page, token_suggestion,
    workspace_has_history,
};
pub use protocol::{
    AgentInfo, AttachmentSource, IngestPhase, IngestProgress, LibraryDocument, LibrarySnapshot,
    PendingAttachment, RagCommand, RagEvent, RagReply, RequestId, SessionInfo, SessionTurn,
    SyncSummary,
};
pub use settings_page::{
    Changes, ExportPrompt, OpenPrompt, PromptActions, no_changes, rag_model_page,
    rag_reference_processing_group, rag_references_group, rag_settings, rag_settings_page,
};

pub(crate) fn count_label(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}
