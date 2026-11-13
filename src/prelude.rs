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
    context_note, control, digest, dirty_color, dirty_header, dropdown_control, expand_home,
    home_relative, missing_markers, number_control, path_control, path_control_home, path_setting,
    prompt_for_paths_at, replace_at, switch_control, text_control, token_control, token_slider,
    verified_bytes,
};

#[cfg(feature = "ragrig")]
pub use crate::rag::{
    AgentInfo, AttachmentSource, Changes, ChatProvider, DEFAULT_GROBID_CONTAINER,
    DEFAULT_GROBID_IMAGE, DEFAULT_GROBID_URL, DEFAULT_LLAMACPP_CONTAINER, DEFAULT_LLAMACPP_IMAGE,
    DEFAULT_OLLAMA_CONTAINER, DEFAULT_OLLAMA_IMAGE, EmbedProvider, ExportPrompt, GrobidSource,
    IngestPhase, IngestProgress, LibraryDocument, LibrarySnapshot, MemoryChoice, OpenPrompt,
    PdfBackend, PendingAttachment, PromptActions, RagChanges, RagChat, RagChatEvent, RagCommand,
    RagController, RagEvent, RagField, RagReply, RagSection, RagSettings, RagSetup, RequestId,
    ScicoAttach, ServiceSource, SessionInfo, SessionTurn, SyncSummary, no_changes, rag_model_page,
    rag_reference_processing_group, rag_references_group, rag_settings, rag_settings_page,
};

#[cfg(feature = "grobid")]
pub use crate::references::{
    CitationAttachment, ReferenceAction, ReferenceCopy, ReferenceEntry, ReferenceOptions,
    ReferenceProgress, ReferenceReport, ReferenceRow, ReferenceSelection, ReferenceSelectionBar,
    ReferenceStage, keys_text, paths_text,
};

#[cfg(feature = "managed")]
pub use crate::services::{
    ContainerStatus, DockerStatus, ExternalServiceEvent, ExternalServices, GROBID_DISCOVERY_PORTS,
    LLAMACPP_DISCOVERY_PORTS, MANAGED_GROBID_CONTAINER, MANAGED_GROBID_PORT, MANAGED_GROBID_URL,
    MANAGED_LLAMACPP_CONTAINER, MANAGED_LLAMACPP_PORT, MANAGED_LLAMACPP_URL,
    MANAGED_OLLAMA_CONTAINER, MANAGED_OLLAMA_PORT, MANAGED_OLLAMA_URL, OLLAMA_DISCOVERY_PORTS,
    Probe, ProbeStatus, ServiceKind, ServiceLaunch, docker_message, existing_url,
    external_services_page, llama_base_url, normalize_grobid_url, normalize_http_url,
    normalize_url, ollama_models, probe,
};
#[cfg(feature = "grobid")]
pub use crate::services::{
    GrobidContainerStatus, GrobidDockerStatus, GrobidProbe, GrobidProbeStatus, GrobidService,
    GrobidServiceEvent,
};
