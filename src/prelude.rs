//! Common imports for hosts of `ragrig-gpui`.
//!
//! ```rust
//! use ragrig_gpui::prelude::*;
//! ```

pub use crate::{
    AgentBar, AgentBarEvent, AgentComponent, Attachment, AttachmentEvent, AttachmentRows,
    ChatMessage, ChatRole, ChatSettings, ChatState, ChatView, ChatViewEvent, ComponentKind,
    ComponentStatus, Composer, ComposerEvent, EmptyState, MessageId, MessageMeta, MessageState,
    NewMessage, SessionEntry, SessionPicker, SessionPickerEvent, SidePanel, SourceRef,
    chat_settings, chat_settings_page, path_setting, token_slider,
};

#[cfg(feature = "ragrig")]
pub use crate::rag::{
    AgentInfo, AttachmentSource, ChatProvider, IngestPhase, IngestProgress, LibraryDocument,
    LibrarySnapshot, MemoryChoice, PdfBackend, PendingAttachment, RagChat, RagChatEvent,
    RagCommand, RagController, RagEvent, RagReply, RagSettings, RagSetup, RequestId,
    SciscraperAttach, SessionInfo, SessionTurn, SyncSummary, rag_model_page, rag_settings,
    rag_settings_page,
};

#[cfg(feature = "grobid")]
pub use crate::rag::rag_references_group;

#[cfg(feature = "grobid")]
pub use crate::references::{
    CitationAttachment, ReferenceAction, ReferenceCopy, ReferenceEntry, ReferenceOptions,
    ReferenceProgress, ReferenceReport, ReferenceRow, ReferenceSelection, ReferenceSelectionBar,
    ReferenceStage, keys_text, paths_text,
};
