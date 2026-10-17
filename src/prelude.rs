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
    chat_settings, chat_settings_page,
};

#[cfg(feature = "ragrig")]
pub use crate::rag::{
    AgentInfo, AttachmentSource, ChatProvider, IngestPhase, IngestProgress, LibraryDocument,
    LibrarySnapshot, MemoryChoice, PdfBackend, PendingAttachment, RagChat, RagChatEvent,
    RagCommand, RagController, RagEvent, RagReply, RagSettings, RagSetup, RequestId, SessionInfo,
    SessionTurn, SyncSummary, rag_settings, rag_settings_page,
};
