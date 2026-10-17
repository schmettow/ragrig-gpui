//! Commands and events exchanged with the Ragrig worker.
//!
//! Everything in this module is plain, UI-friendly data. The worker thread
//! translates [`RagCommand`]s into Ragrig calls and reports [`RagEvent`]s;
//! [`RagController`](crate::rag::RagController) mirrors the parts the widgets
//! need and re-emits every event to interested hosts.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime},
};

use crate::model::{ChatRole, SourceRef};

use super::RagSetup;

/// Identity of one request (generation or ingestion) sent to the worker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RequestId(u64);

impl RequestId {
    pub(crate) fn new(value: u64) -> Self {
        Self(value)
    }

    /// The numeric identity, useful for logging.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "#{}", self.0)
    }
}

/// Where an attached context item comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentSource {
    /// Text already in memory, e.g. an editor selection.
    Text(String),
    /// A file the worker parses before querying.
    File(PathBuf),
}

/// One context item waiting for the next question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingAttachment {
    /// Stable identifier used by the attachment chips.
    pub id: String,
    /// Text shown on the chip.
    pub label: String,
    /// Payload the worker resolves before generation.
    pub source: AttachmentSource,
}

impl PendingAttachment {
    /// An attachment backed by a file path.
    pub fn file(id: impl Into<String>, label: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            source: AttachmentSource::File(path.into()),
        }
    }

    /// An attachment backed by in-memory text.
    pub fn text(id: impl Into<String>, label: impl Into<String>, text: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            source: AttachmentSource::Text(text.into()),
        }
    }
}

/// Work sent from the UI to the Ragrig worker.
#[derive(Clone, Debug)]
pub enum RagCommand {
    /// Ask one question, with one-shot context attachments.
    Ask {
        /// Correlates [`RagEvent::Started`], chunks, and the final event.
        id: RequestId,
        /// Trimmed user prompt.
        prompt: String,
        /// Context that is merged into this prompt only.
        attachments: Vec<PendingAttachment>,
    },
    /// Stop a running generation. The partial answer is kept.
    Stop {
        /// The generation to stop.
        id: RequestId,
    },
    /// Start an empty conversation.
    NewSession,
    /// Continue a saved conversation.
    LoadSession {
        /// Session id from [`RagEvent::Sessions`].
        id: String,
    },
    /// Re-read the session store.
    RefreshSessions,
    /// Re-read the vector store's document list.
    RefreshLibrary,
    /// Incrementally index a folder as a named corpus.
    SyncCorpus {
        /// Correlates the ingestion events.
        id: RequestId,
        /// Corpus identity recorded in chunk provenance.
        name: String,
        /// Folder scanned recursively for supported documents.
        folder: PathBuf,
    },
    /// Stop a running ingestion.
    StopSync {
        /// The ingestion to stop.
        id: RequestId,
    },
    /// Rebuild the agent with new configuration.
    Apply {
        /// The complete configuration and prompt overrides.
        setup: Box<RagSetup>,
    },
}

/// A readable snapshot of a running ingestion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IngestProgress {
    /// Corpus being indexed.
    pub corpus: String,
    /// Document currently being processed.
    pub document: Option<String>,
    /// Zero-based position of the document in the batch.
    pub index: usize,
    /// Number of documents in the batch.
    pub total: usize,
    /// Chunks produced by the current document.
    pub chunks: Option<usize>,
    /// Chunks embedded so far, cumulative.
    pub embedded: usize,
    /// Total chunks to embed, cumulative.
    pub total_chunks: usize,
    /// Coarse phase for the status line.
    pub phase: IngestPhase,
    /// Last per-document error.
    pub failed: Option<String>,
}

impl IngestProgress {
    /// Completion in `0.0..=1.0`, when the total is known.
    pub fn fraction(&self) -> Option<f32> {
        (self.total_chunks > 0).then(|| self.embedded as f32 / self.total_chunks as f32)
    }

    /// One-line description for a status bar.
    pub fn summary(&self) -> String {
        let corpus = if self.corpus.is_empty() {
            "corpus".to_owned()
        } else {
            self.corpus.clone()
        };
        let document = self.document.as_deref().unwrap_or("documents");
        if self.total_chunks > 0 {
            format!(
                "{corpus} · {document} · {}/{} chunks",
                self.embedded, self.total_chunks
            )
        } else if self.total > 0 {
            format!(
                "{corpus} · {document} · {}/{} documents",
                (self.index + 1).min(self.total),
                self.total
            )
        } else {
            format!("{corpus} · {document}")
        }
    }
}

/// Coarse ingestion phase, used for status text and progress emphasis.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IngestPhase {
    /// Discovering and parsing files.
    #[default]
    Starting,
    /// The current document was chunked.
    Parsed,
    /// Chunks are being embedded.
    Embedding,
    /// Chunks were written to the store.
    Stored,
    /// The last document failed.
    Failed,
}

/// Result of a completed ingestion.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncSummary {
    /// Corpus that was synced.
    pub corpus: String,
    /// Documents indexed successfully.
    pub documents: usize,
    /// Chunks written.
    pub chunks: usize,
    /// Documents that failed.
    pub failed: usize,
}

/// One document in the vector store, for the library panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LibraryDocument {
    /// Corpus the document belongs to.
    pub corpus: String,
    /// Document identity within the corpus.
    pub name: String,
    /// Parser that produced its text.
    pub parser: String,
    /// Chunker that split it.
    pub chunker: String,
    /// Filesystem path, resolved from the registered corpus folder.
    pub path: Option<PathBuf>,
}

/// A snapshot of the vector store.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct LibrarySnapshot {
    /// All indexed documents.
    pub documents: Vec<LibraryDocument>,
    /// Total number of stored chunks.
    pub chunks: usize,
}

/// One saved conversation, before the controller formats it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionInfo {
    /// Session identifier.
    pub id: String,
    /// Number of recorded turns.
    pub turn_count: usize,
    /// Optional one-line summary produced by a history strategy.
    pub summary: Option<String>,
    /// Last modification time.
    pub updated: SystemTime,
}

/// A conversation turn loaded from the session store.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionTurn {
    /// Speaker.
    pub role: ChatRole,
    /// Full text.
    pub text: String,
}

/// Metadata returned with a finished answer.
#[derive(Clone, Debug, PartialEq)]
pub struct RagReply {
    /// The generated answer.
    pub answer: String,
    /// Model that produced it.
    pub model: String,
    /// Number of chunks that entered the prompt.
    pub chunks: Option<usize>,
    /// Distinct documents among the retrieved chunks.
    pub sources: Vec<SourceRef>,
    /// Wall-clock duration of the full pipeline.
    pub elapsed: Option<Duration>,
    /// Search query after optional rewriting.
    pub rewritten_query: Option<String>,
}

/// The active Ragrig agent, as reported after a successful build.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentInfo {
    /// Workspace holding the store and sessions.
    pub workspace: PathBuf,
    /// Chat backend name.
    pub chat_backend: String,
    /// Chat model in use.
    pub chat_model: String,
    /// Embedding backend name.
    pub embed_backend: String,
    /// Embedding model in use.
    pub embed_model: String,
    /// Memory strategy description.
    pub memory: String,
    /// Primary PDF parser backend.
    pub pdf_backend: String,
    /// Retrieval budget.
    pub top_k: usize,
    /// Chunk size in tokens.
    pub chunk_size: usize,
    /// Chunk overlap in tokens.
    pub chunk_overlap: usize,
    /// Stored chunks at build time.
    pub chunks: usize,
    /// Stored documents at build time.
    pub documents: usize,
}

/// Events emitted by the worker and re-emitted by the controller.
#[derive(Clone, Debug)]
pub enum RagEvent {
    /// The agent was built and is ready for questions.
    AgentReady(AgentInfo),
    /// The agent could not be built.
    AgentFailed {
        /// Error chain.
        message: String,
    },
    /// A fresh conversation started.
    SessionStarted {
        /// New session id.
        id: String,
    },
    /// A saved conversation was loaded.
    SessionLoaded {
        /// Session id.
        id: String,
        /// Recorded turns, in order.
        turns: Vec<SessionTurn>,
    },
    /// The session store was re-read.
    Sessions(Vec<SessionInfo>),
    /// Generation started.
    Started {
        /// Request identity.
        id: RequestId,
    },
    /// A batch of streamed tokens.
    Chunk {
        /// Request identity.
        id: RequestId,
        /// Token batch.
        delta: String,
    },
    /// Generation finished successfully.
    Finished {
        /// Request identity.
        id: RequestId,
        /// Answer and retrieval metadata.
        reply: RagReply,
    },
    /// Generation was cancelled; the partial answer is kept.
    Stopped {
        /// Request identity.
        id: RequestId,
        /// Text produced before cancellation.
        partial: String,
    },
    /// Generation failed.
    Failed {
        /// Request identity.
        id: RequestId,
        /// Error chain.
        message: String,
    },
    /// The vector store was re-read.
    Library(LibrarySnapshot),
    /// Ingestion started.
    SyncStarted {
        /// Request identity.
        id: RequestId,
        /// Corpus name.
        corpus: String,
    },
    /// Ingestion progress.
    SyncProgress {
        /// Request identity.
        id: RequestId,
        /// Current snapshot.
        progress: IngestProgress,
    },
    /// Ingestion finished.
    SyncFinished {
        /// Request identity.
        id: RequestId,
        /// Per-run totals.
        summary: SyncSummary,
    },
    /// Ingestion was cancelled.
    SyncStopped {
        /// Request identity.
        id: RequestId,
    },
    /// Ingestion failed.
    SyncFailed {
        /// Request identity.
        id: RequestId,
        /// Error chain.
        message: String,
    },
    /// A non-fatal notice for the host's status line.
    Notice(String),
}
