//! The GPUI-side controller: commands in, Ragrig events out.
//!
//! [`RagController`] owns the worker thread, mirrors the state widgets need
//! (sessions, library, component status), and re-emits every [`RagEvent`].
//! It never runs Ragrig futures on GPUI's executor.

use std::{
    path::PathBuf,
    thread,
    time::{Duration, SystemTime},
};

use async_channel::{Sender, TrySendError};
use gpui_kit::{Context, EventEmitter};

use crate::model::{AgentComponent, ComponentKind, ComponentStatus, SessionEntry};
#[cfg(feature = "grobid")]
use crate::references::{ReferenceEntry, ReferenceOptions, ReferenceProgress, ReferenceReport};

use super::{
    AgentInfo, IngestProgress, LibrarySnapshot, PendingAttachment, RagCommand, RagEvent, RagSetup,
    RequestId, SessionInfo, actor, count_label,
};

/// Sends commands to the Ragrig worker and mirrors its state for widgets.
pub struct RagController {
    commands: Sender<RagCommand>,
    worker: Option<thread::JoinHandle<()>>,
    sessions: Vec<SessionEntry>,
    current_session: Option<String>,
    library: LibrarySnapshot,
    agent: Option<AgentInfo>,
    ingest: Option<IngestProgress>,
    sync_request: Option<RequestId>,
    busy: bool,
    last_error: Option<String>,
    last_notice: Option<String>,
    next_request: u64,
    #[cfg(feature = "grobid")]
    references: Vec<ReferenceEntry>,
    #[cfg(feature = "grobid")]
    reference_bibliography: Option<PathBuf>,
    #[cfg(feature = "grobid")]
    reference_report: Option<ReferenceReport>,
    #[cfg(feature = "grobid")]
    reference_progress: Option<ReferenceProgress>,
    #[cfg(feature = "grobid")]
    reference_request: Option<RequestId>,
    #[cfg(feature = "grobid")]
    reference_folder: Option<PathBuf>,
}

impl RagController {
    /// Spawn the worker and start mirroring its events.
    ///
    /// Returns an error when the worker thread cannot be created; an agent
    /// that fails to build is reported later through
    /// [`RagEvent::AgentFailed`].
    pub fn new(setup: RagSetup, cx: &mut Context<Self>) -> Result<Self, String> {
        let (commands, command_rx) = async_channel::bounded(32);
        let (events_tx, events) = async_channel::bounded(256);
        let worker = actor::start(setup, None, command_rx, events_tx)
            .map_err(|error| format!("cannot start the RAG worker: {error}"))?;

        cx.spawn(async move |this, cx| {
            while let Ok(event) = events.recv().await {
                if this
                    .update(cx, |controller, cx| controller.on_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        let controller = Self {
            commands,
            worker: Some(worker),
            sessions: Vec::new(),
            current_session: None,
            library: LibrarySnapshot::default(),
            agent: None,
            ingest: None,
            sync_request: None,
            busy: false,
            last_error: None,
            last_notice: None,
            next_request: 0,
            #[cfg(feature = "grobid")]
            references: Vec::new(),
            #[cfg(feature = "grobid")]
            reference_bibliography: None,
            #[cfg(feature = "grobid")]
            reference_report: None,
            #[cfg(feature = "grobid")]
            reference_progress: None,
            #[cfg(feature = "grobid")]
            reference_request: None,
            #[cfg(feature = "grobid")]
            reference_folder: None,
        };
        // The worker announces ready, the library, and the session list as
        // soon as its agent is built.
        Ok(controller)
    }

    /// Ask one question and return the request identity.
    pub fn ask(
        &mut self,
        prompt: impl Into<String>,
        attachments: Vec<PendingAttachment>,
        cx: &mut Context<Self>,
    ) -> Result<RequestId, String> {
        self.ask_with_retrieval(prompt, None, attachments, cx)
    }

    /// Ask one question whose retrieval is enriched with `retrieval`.
    ///
    /// The worker prepends `retrieval` to the prompt before embedding and
    /// searching, so a marked passage drives the similarity search as well as
    /// the question. An empty or whitespace-only enrichment is ignored. The
    /// host transcript still echoes only the prompt.
    pub fn ask_with_retrieval(
        &mut self,
        prompt: impl Into<String>,
        retrieval: Option<String>,
        attachments: Vec<PendingAttachment>,
        cx: &mut Context<Self>,
    ) -> Result<RequestId, String> {
        let prompt = prompt.into();
        let prompt = prompt.trim().to_owned();
        if prompt.is_empty() {
            return Err("enter a question first".to_owned());
        }
        let retrieval = retrieval
            .map(|text| text.trim().to_owned())
            .filter(|text| !text.is_empty());
        let id = self.request_id();
        self.send(
            RagCommand::Ask {
                id,
                prompt,
                retrieval,
                attachments,
            },
            cx,
        )?;
        Ok(id)
    }

    /// Stop one running generation.
    pub fn stop(&mut self, id: RequestId, cx: &mut Context<Self>) {
        self.send_quiet(RagCommand::Stop { id }, cx);
    }

    /// Start an empty conversation.
    pub fn new_session(&mut self, cx: &mut Context<Self>) {
        self.send_quiet(RagCommand::NewSession, cx);
    }

    /// Continue a saved conversation.
    pub fn load_session(&mut self, id: impl Into<String>, cx: &mut Context<Self>) {
        self.send_quiet(RagCommand::LoadSession { id: id.into() }, cx);
    }

    /// Re-read the session store.
    pub fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        self.send_quiet(RagCommand::RefreshSessions, cx);
    }

    /// Re-read the vector store.
    pub fn refresh_library(&mut self, cx: &mut Context<Self>) {
        self.send_quiet(RagCommand::RefreshLibrary, cx);
    }

    /// Index a folder as a named corpus.
    pub fn sync_corpus(
        &mut self,
        name: impl Into<String>,
        folder: impl Into<PathBuf>,
        cx: &mut Context<Self>,
    ) -> Result<RequestId, String> {
        let id = self.request_id();
        self.send(
            RagCommand::SyncCorpus {
                id,
                name: name.into(),
                folder: folder.into(),
            },
            cx,
        )?;
        Ok(id)
    }

    /// Stop the running ingestion.
    pub fn stop_sync(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.sync_request {
            self.send_quiet(RagCommand::StopSync { id }, cx);
        }
    }

    /// Parse `folder` with GROBID and merge fresh records into the workspace
    /// bibliography.
    #[cfg(feature = "grobid")]
    pub fn extract_references(
        &mut self,
        folder: impl Into<PathBuf>,
        options: ReferenceOptions,
        cx: &mut Context<Self>,
    ) -> Result<RequestId, String> {
        let folder = folder.into();
        let id = self.request_id();
        self.reference_folder = Some(folder.clone());
        self.send(
            RagCommand::ExtractReferences {
                id,
                folder,
                options,
            },
            cx,
        )?;
        Ok(id)
    }

    /// Stop the running reference extraction.
    #[cfg(feature = "grobid")]
    pub fn stop_references(&mut self, cx: &mut Context<Self>) {
        if let Some(id) = self.reference_request {
            self.send_quiet(RagCommand::StopReferences { id }, cx);
        }
    }

    /// Load the workspace bibliography without contacting GROBID.
    #[cfg(feature = "grobid")]
    pub fn refresh_references(&mut self, folder: impl Into<PathBuf>, cx: &mut Context<Self>) {
        let folder = folder.into();
        self.reference_folder = Some(folder.clone());
        self.send_quiet(RagCommand::RefreshReferences { folder }, cx);
    }

    /// Rebuild the agent with new configuration.
    pub fn apply(&mut self, setup: RagSetup, cx: &mut Context<Self>) {
        self.agent = None;
        self.busy = false;
        self.ingest = None;
        self.sync_request = None;
        self.current_session = None;
        self.library = LibrarySnapshot::default();
        self.last_error = None;
        self.last_notice = None;
        #[cfg(feature = "grobid")]
        let reference_folder = self.reference_folder.clone();
        #[cfg(feature = "grobid")]
        {
            self.references.clear();
            self.reference_bibliography = None;
            self.reference_report = None;
            self.reference_progress = None;
            self.reference_request = None;
        }
        self.send_quiet(
            RagCommand::Apply {
                setup: Box::new(setup),
            },
            cx,
        );
        #[cfg(feature = "grobid")]
        if let Some(folder) = reference_folder {
            self.send_quiet(RagCommand::RefreshReferences { folder }, cx);
        }
    }

    /// Saved conversations, newest first.
    pub fn sessions(&self) -> &[SessionEntry] {
        &self.sessions
    }

    /// The active conversation id, when known.
    pub fn current_session(&self) -> Option<&str> {
        self.current_session.as_deref()
    }

    /// The last library snapshot.
    pub fn library(&self) -> &LibrarySnapshot {
        &self.library
    }

    /// The active agent, once it built successfully.
    pub fn agent(&self) -> Option<&AgentInfo> {
        self.agent.as_ref()
    }

    /// The running ingestion, if any.
    pub fn ingest_progress(&self) -> Option<&IngestProgress> {
        self.ingest.as_ref()
    }

    /// Whether an answer is streaming.
    pub fn is_busy(&self) -> bool {
        self.busy
    }

    /// Whether a corpus is being indexed.
    pub fn is_ingesting(&self) -> bool {
        self.ingest.is_some()
    }

    /// Reference rows loaded from the workspace bibliography.
    #[cfg(feature = "grobid")]
    pub fn references(&self) -> &[ReferenceEntry] {
        &self.references
    }

    /// The `.bib` file backing the reference rows.
    #[cfg(feature = "grobid")]
    pub fn reference_bibliography(&self) -> Option<&std::path::Path> {
        self.reference_bibliography.as_deref()
    }

    /// What the last reference extraction did.
    #[cfg(feature = "grobid")]
    pub fn reference_report(&self) -> Option<&ReferenceReport> {
        self.reference_report.as_ref()
    }

    /// Progress of the running reference extraction.
    #[cfg(feature = "grobid")]
    pub fn reference_progress(&self) -> Option<&ReferenceProgress> {
        self.reference_progress.as_ref()
    }

    /// Whether GROBID is currently parsing a corpus.
    #[cfg(feature = "grobid")]
    pub fn is_extracting_references(&self) -> bool {
        self.reference_progress.is_some()
    }

    /// The last fatal error.
    pub fn error(&self) -> Option<&str> {
        self.last_error.as_deref()
    }

    /// One line for a status bar.
    pub fn status_line(&self) -> String {
        if let Some(error) = &self.last_error {
            return error.clone();
        }
        if let Some(progress) = &self.ingest {
            return progress.summary();
        }
        #[cfg(feature = "grobid")]
        if let Some(progress) = &self.reference_progress {
            return progress.summary();
        }
        if self.busy {
            return "Generating…".to_owned();
        }
        match &self.agent {
            Some(agent) => format!(
                "{} · {} · {}",
                agent.chat_model,
                count_label(agent.documents, "document", "documents"),
                count_label(agent.chunks, "chunk", "chunks")
            ),
            None => self
                .last_notice
                .clone()
                .unwrap_or_else(|| "Starting the RAG worker…".to_owned()),
        }
    }

    /// Pipeline components for [`AgentBar`](crate::AgentBar).
    pub fn agent_components(&self) -> Vec<AgentComponent> {
        let Some(agent) = &self.agent else {
            let status = if self.last_error.is_some() {
                ComponentStatus::Error
            } else {
                ComponentStatus::Inactive
            };
            let components: Vec<AgentComponent> = [
                (ComponentKind::Chat, "Chat model"),
                (ComponentKind::Embedder, "Embedder"),
                (ComponentKind::Store, "Vector store"),
                (ComponentKind::Parser, "PDF parser"),
                (ComponentKind::Chunker, "Chunker"),
                (ComponentKind::Retriever, "Retriever"),
            ]
            .into_iter()
            .map(|(kind, label)| AgentComponent::new(kind_label(kind), kind, label).status(status))
            .collect();
            #[cfg(feature = "grobid")]
            let components = {
                let mut components = components;
                components.push(
                    AgentComponent::new("references", ComponentKind::References, "GROBID")
                        .status(status),
                );
                components
            };
            return components;
        };

        let pipeline = if self.ingest.is_some() {
            ComponentStatus::Busy
        } else {
            ComponentStatus::Ready
        };
        let chat = if self.busy {
            ComponentStatus::Busy
        } else {
            ComponentStatus::Ready
        };

        let components = vec![
            AgentComponent::new("chat", ComponentKind::Chat, agent.chat_model.clone())
                .status(chat)
                .detail(format!("{} · {} memory", agent.chat_backend, agent.memory)),
            AgentComponent::new("embed", ComponentKind::Embedder, agent.embed_model.clone())
                .status(pipeline)
                .detail(format!("{} · top {}", agent.embed_backend, agent.top_k)),
            AgentComponent::new("store", ComponentKind::Store, "Vector store")
                .status(pipeline)
                .detail(format!(
                    "{} · {}",
                    count_label(agent.documents, "document", "documents"),
                    count_label(agent.chunks, "chunk", "chunks")
                )),
            AgentComponent::new("parser", ComponentKind::Parser, agent.pdf_backend.clone())
                .status(pipeline)
                .detail("PDF and Markdown"),
            AgentComponent::new("chunker", ComponentKind::Chunker, "Markdown")
                .status(pipeline)
                .detail(format!(
                    "{} tokens · {} overlap",
                    agent.chunk_size, agent.chunk_overlap
                )),
            AgentComponent::new("retriever", ComponentKind::Retriever, "Hybrid retrieval")
                .status(pipeline)
                .detail(format!("top {}", agent.top_k)),
        ];
        #[cfg(feature = "grobid")]
        let components = {
            let mut components = components;
            let status = if self.reference_progress.is_some() {
                ComponentStatus::Busy
            } else if self.references.is_empty() {
                ComponentStatus::Inactive
            } else {
                ComponentStatus::Ready
            };
            let detail = match &self.reference_bibliography {
                Some(path) => format!(
                    "{} · {}",
                    count_label(self.references.len(), "reference", "references"),
                    path.file_name()
                        .and_then(|name| name.to_str())
                        .unwrap_or("references.bib")
                ),
                None => count_label(self.references.len(), "reference", "references"),
            };
            components.push(
                AgentComponent::new("references", ComponentKind::References, "GROBID")
                    .status(status)
                    .detail(detail),
            );
            components
        };
        components
    }

    fn request_id(&mut self) -> RequestId {
        self.next_request = self.next_request.wrapping_add(1);
        RequestId::new(self.next_request)
    }

    fn send(&self, command: RagCommand, cx: &mut Context<Self>) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                TrySendError::Full(_) => "the RAG worker is busy; try again",
                TrySendError::Closed(_) => "the RAG worker stopped",
            })?;
        cx.notify();
        Ok(())
    }

    fn send_quiet(&mut self, command: RagCommand, cx: &mut Context<Self>) {
        if let Err(message) = self.send(command, cx) {
            self.last_error = Some(message);
            cx.notify();
        }
    }

    fn on_event(&mut self, event: RagEvent, cx: &mut Context<Self>) {
        match &event {
            RagEvent::AgentReady(info) => {
                self.agent = Some(info.clone());
                self.last_error = None;
            }
            RagEvent::AgentFailed { message } => {
                self.agent = None;
                self.busy = false;
                self.last_error = Some(message.clone());
            }
            RagEvent::SessionStarted { id } | RagEvent::SessionLoaded { id, .. } => {
                self.current_session = Some(id.clone());
            }
            RagEvent::Sessions(sessions) => {
                self.sessions = session_entries(sessions, SystemTime::now());
            }
            RagEvent::Started { .. } => {
                self.busy = true;
                self.last_notice = None;
                self.last_error = None;
            }
            RagEvent::Chunk { .. } => {}
            RagEvent::Finished { .. } | RagEvent::Stopped { .. } => {
                self.busy = false;
            }
            RagEvent::Failed { message, .. } => {
                self.busy = false;
                self.last_error = Some(message.clone());
            }
            RagEvent::Library(snapshot) => {
                self.library = snapshot.clone();
                if let Some(agent) = &mut self.agent {
                    agent.chunks = snapshot.chunks;
                    agent.documents = snapshot.documents.len();
                }
            }
            #[cfg(feature = "grobid")]
            RagEvent::Citations { .. } => {}
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesStarted { id, folder } => {
                self.reference_request = Some(*id);
                self.reference_folder = Some(folder.clone());
                self.reference_progress = Some(ReferenceProgress::default());
                self.last_error = None;
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesProgress { progress, .. } => {
                self.reference_progress = Some(progress.clone());
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesLoaded {
                entries,
                bibliography,
            } => {
                self.references = entries.clone();
                self.reference_bibliography = Some(bibliography.clone());
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesFinished { report, .. } => {
                self.reference_report = Some(report.clone());
                self.reference_progress = None;
                self.reference_request = None;
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesStopped { .. } => {
                self.reference_progress = None;
                self.reference_request = None;
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesFailed { message, .. } => {
                self.reference_progress = None;
                self.reference_request = None;
                self.last_error = Some(message.clone());
            }
            RagEvent::SyncStarted { id, corpus } => {
                self.sync_request = Some(*id);
                self.last_error = None;
                self.ingest = Some(IngestProgress {
                    corpus: corpus.clone(),
                    ..IngestProgress::default()
                });
            }
            RagEvent::SyncProgress { progress, .. } => {
                self.ingest = Some(progress.clone());
            }
            RagEvent::SyncFinished { .. } | RagEvent::SyncStopped { .. } => {
                self.ingest = None;
                self.sync_request = None;
            }
            RagEvent::SyncFailed { message, .. } => {
                self.ingest = None;
                self.sync_request = None;
                self.last_error = Some(message.clone());
            }
            RagEvent::Notice(message) => {
                self.last_notice = Some(message.clone());
            }
        }

        cx.notify();
        cx.emit(event);
    }
}

impl EventEmitter<RagEvent> for RagController {}

impl Drop for RagController {
    fn drop(&mut self) {
        self.commands.close();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn kind_label(kind: ComponentKind) -> &'static str {
    match kind {
        ComponentKind::Chat => "chat",
        ComponentKind::Embedder => "embed",
        ComponentKind::Store => "store",
        ComponentKind::Parser => "parser",
        ComponentKind::Chunker => "chunker",
        ComponentKind::Retriever => "retriever",
        ComponentKind::Vision => "vision",
        ComponentKind::References => "references",
        ComponentKind::Other => "component",
    }
}

fn session_entries(sessions: &[SessionInfo], now: SystemTime) -> Vec<SessionEntry> {
    let mut sessions = sessions.to_vec();
    sessions.sort_by_key(|session| std::cmp::Reverse(session.updated));
    sessions
        .into_iter()
        .map(|session| {
            let label = session
                .summary
                .as_deref()
                .map(str::trim)
                .filter(|summary| !summary.is_empty())
                .map(str::to_owned)
                .unwrap_or_else(|| match session.turn_count {
                    0 => "Empty conversation".to_owned(),
                    1 => "1 turn".to_owned(),
                    count => format!("{count} turns"),
                });
            SessionEntry::new(session.id, label).detail(relative_time(now, session.updated))
        })
        .collect()
}

fn relative_time(now: SystemTime, then: SystemTime) -> String {
    let elapsed = now.duration_since(then).unwrap_or_default();
    if elapsed < Duration::from_secs(60) {
        "just now".to_owned()
    } else if elapsed < Duration::from_secs(3_600) {
        format!("{} min ago", elapsed.as_secs() / 60)
    } else if elapsed < Duration::from_secs(86_400) {
        format!("{} h ago", elapsed.as_secs() / 3_600)
    } else {
        format!("{} d ago", elapsed.as_secs() / 86_400)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, turns: usize, summary: Option<&str>, updated: u64) -> SessionInfo {
        SessionInfo {
            id: id.to_owned(),
            turn_count: turns,
            summary: summary.map(str::to_owned),
            updated: SystemTime::UNIX_EPOCH + Duration::from_secs(updated),
        }
    }

    #[test]
    fn sessions_are_sorted_and_labelled() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let entries = session_entries(
            &[
                info("old", 1, None, 1_000),
                info("new", 3, None, 9_000),
                info("summary", 0, Some("Methods review"), 5_000),
            ],
            now,
        );
        assert_eq!(entries[0].id, "new");
        assert_eq!(entries[0].label, "3 turns");
        assert_eq!(entries[1].id, "summary");
        assert_eq!(entries[1].label, "Methods review");
        assert_eq!(entries[2].id, "old");
        assert_eq!(entries[2].label, "1 turn");
    }

    #[test]
    fn relative_times_are_human_readable() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(100_000);
        assert_eq!(relative_time(now, now - Duration::from_secs(5)), "just now");
        assert_eq!(
            relative_time(now, now - Duration::from_secs(120)),
            "2 min ago"
        );
        assert_eq!(
            relative_time(now, now - Duration::from_secs(7_200)),
            "2 h ago"
        );
        assert_eq!(
            relative_time(now, now - Duration::from_secs(172_800)),
            "2 d ago"
        );
    }
}
