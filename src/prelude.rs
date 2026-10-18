//! Common imports for hosts of `ragrig-gpui`.
//!
//! ```rust
//! use ragrig_gpui::prelude::*;
//! ```

pub use crate::{
    AgentBar, AgentBarEvent, AgentComponent, Attachment, AttachmentEvent, AttachmentRows,
    ChatMessage, ChatRole, ChatSettings, ChatState, ChatView, ChatViewEvent, ComponentKind,
    ComponentStatus, Composer, ComposerEvent, Control, EmptyState, MessageId, MessageMeta,
    MessageState, NewMessage, SessionEntry, SessionPicker, SessionPickerEvent, SettingRow,
    SidePanel, SourceRef, chat_settings, chat_settings_page, control, dirty_color, dirty_header,
    dropdown_control, number_control, path_control, path_setting, switch_control, text_control,
    token_control, token_slider,
};

#[cfg(feature = "ragrig")]
pub use crate::rag::{
    AgentInfo, AttachmentSource, Changes, ChatProvider, IngestPhase, IngestProgress,
    LibraryDocument, LibrarySnapshot, MemoryChoice, PdfBackend, PendingAttachment, RagChanges,
    RagChat, RagChatEvent, RagCommand, RagController, RagEvent, RagField, RagReply, RagSection,
    RagSettings, RagSetup, RequestId, SciscraperAttach, SessionInfo, SessionTurn, SyncSummary,
    no_changes, rag_model_page, rag_references_group, rag_settings, rag_settings_page,
};

#[cfg(feature = "grobid")]
pub use crate::references::{
    CitationAttachment, ReferenceAction, ReferenceCopy, ReferenceEntry, ReferenceOptions,
    ReferenceProgress, ReferenceReport, ReferenceRow, ReferenceSelection, ReferenceSelectionBar,
    ReferenceStage, keys_text, paths_text,
};
