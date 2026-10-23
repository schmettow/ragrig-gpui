//! Common imports for hosts of `ragrig-gpui`.
//!
//! ```rust
//! use ragrig_gpui::prelude::*;
//! ```

pub use crate::{
    AgentBar, AgentBarEvent, AgentComponent, Attachment, AttachmentEvent, AttachmentRows, ChatIcon,
    ChatIdentity, ChatMessage, ChatRole, ChatSettings, ChatState, ChatView, ChatViewEvent,
    ComponentKind, ComponentStatus, ComponentToggle, Composer, ComposerEvent, ContextNote, Control,
    EditorBuffer, EditorSnapshot, EditorSurface, EmptyState, MARKER_CONTEXT, MARKER_DOCUMENT,
    MARKER_LIBRARY, MARKER_REPLACE, MARKER_SELECTION, MARKER_TRUNCATED, MARKERS, MAX_CONTEXT_CHARS,
    MessageId, MessageMeta, MessageState, NewMessage, PROTOCOL, Replacement, ReplacementOutcome,
    ReplacementTarget, Roundtrip, RoundtripContext, RoundtripHost, RoundtripKind, SelectionContext,
    SelectionSnapshot, SessionEntry, SessionPicker, SessionPickerEvent, SettingRow, SidePanel,
    SourceRef, byte_to_char_index, char_range_to_bytes, chat_settings, chat_settings_page,
    context_note, control, digest, dirty_color, dirty_header, dropdown_control, missing_markers,
    number_control, path_control, path_setting, replace_at, switch_control, text_control,
    token_control, token_slider, verified_bytes,
};

#[cfg(feature = "ragrig")]
pub use crate::rag::{
    AgentInfo, AttachmentSource, Changes, ChatProvider, DEFAULT_GROBID_IMAGE, DEFAULT_GROBID_URL,
    ExportPrompt, GrobidSource, IngestPhase, IngestProgress, LibraryDocument, LibrarySnapshot,
    MemoryChoice, OpenPrompt, PdfBackend, PendingAttachment, PromptActions, RagChanges, RagChat,
    RagChatEvent, RagCommand, RagController, RagEvent, RagField, RagReply, RagSection, RagSettings,
    RagSetup, RequestId, SciscraperAttach, SessionInfo, SessionTurn, SyncSummary, no_changes,
    rag_model_page, rag_reference_processing_group, rag_references_group, rag_settings,
    rag_settings_page,
};

#[cfg(feature = "grobid")]
pub use crate::references::{
    CitationAttachment, ReferenceAction, ReferenceCopy, ReferenceEntry, ReferenceOptions,
    ReferenceProgress, ReferenceReport, ReferenceRow, ReferenceSelection, ReferenceSelectionBar,
    ReferenceStage, keys_text, paths_text,
};

#[cfg(feature = "grobid")]
pub use crate::services::{
    GROBID_DISCOVERY_PORTS, GrobidContainerStatus, GrobidDockerStatus, GrobidProbe,
    GrobidProbeStatus, GrobidService, GrobidServiceEvent, MANAGED_GROBID_CONTAINER,
    MANAGED_GROBID_PORT, MANAGED_GROBID_URL, external_services_page, normalize_grobid_url,
};
