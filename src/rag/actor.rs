//! The dedicated Tokio worker that owns one [`AgentSession`].
//!
//! Nothing in this module touches GPUI. The thread builds a multi-thread
//! Tokio runtime, constructs the Ragrig agent, and processes commands until
//! every sender is dropped. Streaming tokens are batched on the worker side
//! (about 30 Hz) so the UI never handles one wakeup per token.

use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use async_channel::{Receiver, Sender, TrySendError};
use ragrig::progress::Cancel as _;
use ragrig::{
    Cancelled, ChatAgentSpec, ChunkConfig, DocumentParsers, EmbedderSpec, FolderCorpus,
    FsSessionStore, LogHistory, MemoryStrategyKind, ProgressEvent, RagAgent, RagResponse,
    SummaryHistory, build_parsers,
    parsers::extract_text,
    progress::CancellationToken,
    scan_document_files,
    session::AgentSession,
    store,
    types::{AttachedDocument, ParseConfig, Provider},
};

use crate::model::SourceRef;
#[cfg(feature = "grobid")]
use crate::references::{
    self, CitationResolution, ExtractionError, ExtractionObserver, ReferenceOptions,
    ReferenceProgress,
};
#[cfg(feature = "grobid")]
use grobid_bibtex::cite::CiteState;

use super::{
    AgentInfo, AttachmentSource, IngestPhase, IngestProgress, LibraryDocument, LibrarySnapshot,
    PendingAttachment, RagCommand, RagEvent, RagReply, RagSetup, RequestId, ScicoAttach,
    SessionInfo, SessionTurn, SyncSummary,
};

/// How often buffered tokens are flushed to the UI.
const TOKEN_BATCH: Duration = Duration::from_millis(33);

fn trace(message: impl AsRef<str>) {
    if std::env::var_os("RAGRIG_GPUI_TRACE").is_some() {
        eprintln!("[ragrig-gpui/worker] {}", message.as_ref());
    }
}

type SessionSlot = tokio::sync::Mutex<Option<AgentSession>>;

struct Shared {
    session: SessionSlot,
    setup: Mutex<RagSetup>,
    corpora: Mutex<HashMap<String, PathBuf>>,
    cancels: Mutex<HashMap<RequestId, CancellationToken>>,
    generating: AtomicBool,
    syncing: AtomicBool,
    #[cfg(feature = "grobid")]
    referencing: AtomicBool,
    /// Bibliography view cached across questions; invalidated by file changes.
    #[cfg(feature = "grobid")]
    citations: Mutex<CiteState>,
}

/// Start the worker thread.
///
/// `initial` is used by tests to inject a session with fake backends; the
/// controller always passes `None`, which makes the worker build the agent
/// from `setup`.
pub(crate) fn start(
    setup: RagSetup,
    initial: Option<AgentSession>,
    commands: Receiver<RagCommand>,
    events: Sender<RagEvent>,
) -> std::io::Result<thread::JoinHandle<()>> {
    thread::Builder::new()
        .name("ragrig-gpui-worker".to_owned())
        .spawn(move || {
            trace("thread started");
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    let _ = events.send_blocking(RagEvent::AgentFailed {
                        message: format!("cannot start the Tokio runtime: {error}"),
                    });
                    return;
                }
            };

            runtime.block_on(run(setup, initial, commands, events));
        })
}

async fn run(
    setup: RagSetup,
    initial: Option<AgentSession>,
    commands: Receiver<RagCommand>,
    events: Sender<RagEvent>,
) {
    let corpora = parse_corpora(&setup.config.corpus_dirs);
    let shared = Arc::new(Shared {
        session: tokio::sync::Mutex::new(initial),
        setup: Mutex::new(setup),
        corpora: Mutex::new(corpora),
        cancels: Mutex::new(HashMap::new()),
        generating: AtomicBool::new(false),
        syncing: AtomicBool::new(false),
        #[cfg(feature = "grobid")]
        referencing: AtomicBool::new(false),
        #[cfg(feature = "grobid")]
        citations: Mutex::new(CiteState::default()),
    });

    if shared.session.lock().await.is_none() {
        rebuild_session(&shared, &events, None).await;
    }
    announce_ready(&shared, &events).await;

    trace("entering command loop");
    while let Ok(command) = commands.recv().await {
        let shared = Arc::clone(&shared);
        handle(command, shared, events.clone()).await;
    }
}

async fn handle(command: RagCommand, shared: Arc<Shared>, events: Sender<RagEvent>) {
    match command {
        RagCommand::Ask {
            id,
            prompt,
            retrieval,
            attachments,
        } => {
            if shared.syncing.load(Ordering::Acquire) {
                let _ = events
                    .send(RagEvent::Failed {
                        id,
                        message: "wait for the ingestion to finish".to_owned(),
                    })
                    .await;
                return;
            }
            if shared.generating.swap(true, Ordering::AcqRel) {
                let _ = events
                    .send(RagEvent::Failed {
                        id,
                        message: "a generation is already running".to_owned(),
                    })
                    .await;
                return;
            }
            let cancel = CancellationToken::new();
            shared
                .cancels
                .lock()
                .expect("cancel map")
                .insert(id, cancel.clone());
            tokio::spawn(run_ask(
                shared,
                id,
                prompt,
                retrieval,
                attachments,
                cancel,
                events,
            ));
        }
        RagCommand::Stop { id } => {
            if let Some(cancel) = shared.cancels.lock().expect("cancel map").remove(&id) {
                cancel.cancel();
            }
        }
        RagCommand::NewSession => {
            let mut guard = shared.session.lock().await;
            if let Some(session) = guard.as_mut() {
                session.new_session();
                let id = session.session_id().0.clone();
                drop(guard);
                let _ = events.send(RagEvent::SessionStarted { id }).await;
            } else {
                drop(guard);
                let _ = events
                    .send(RagEvent::Notice(
                        "no agent is ready yet; check the settings".to_owned(),
                    ))
                    .await;
            }
        }
        RagCommand::LoadSession { id } => {
            let mut guard = shared.session.lock().await;
            let Some(session) = guard.as_mut() else {
                drop(guard);
                let _ = events
                    .send(RagEvent::Notice(
                        "no agent is ready yet; check the settings".to_owned(),
                    ))
                    .await;
                return;
            };
            match session.load_session(&ragrig::SessionId(id.clone())).await {
                Ok(true) => {
                    let turns = session_turns(session);
                    drop(guard);
                    let _ = events.send(RagEvent::SessionLoaded { id, turns }).await;
                }
                Ok(false) => {
                    drop(guard);
                    let _ = events
                        .send(RagEvent::Notice(format!("session '{id}' no longer exists")))
                        .await;
                }
                Err(error) => {
                    drop(guard);
                    let _ = events
                        .send(RagEvent::Notice(format!("cannot load session: {error:#}")))
                        .await;
                }
            }
        }
        RagCommand::RefreshSessions => send_sessions(&shared, &events).await,
        RagCommand::RefreshLibrary => send_library(&shared, &events).await,
        RagCommand::SyncCorpus { id, name, folder } => {
            if shared.generating.load(Ordering::Acquire) {
                let _ = events
                    .send(RagEvent::SyncFailed {
                        id,
                        message: "stop the running answer before indexing".to_owned(),
                    })
                    .await;
                return;
            }
            if shared.syncing.swap(true, Ordering::AcqRel) {
                let _ = events
                    .send(RagEvent::SyncFailed {
                        id,
                        message: "an ingestion is already running".to_owned(),
                    })
                    .await;
                return;
            }
            shared
                .corpora
                .lock()
                .expect("corpus map")
                .insert(name.clone(), folder.clone());
            let cancel = CancellationToken::new();
            shared
                .cancels
                .lock()
                .expect("cancel map")
                .insert(id, cancel.clone());
            tokio::spawn(run_sync(shared, id, name, folder, cancel, events));
        }
        RagCommand::StopSync { id } => {
            if let Some(cancel) = shared.cancels.lock().expect("cancel map").remove(&id) {
                cancel.cancel();
            }
        }
        #[cfg(feature = "grobid")]
        RagCommand::ExtractReferences {
            id,
            folder,
            options,
        } => {
            if shared.referencing.swap(true, Ordering::AcqRel) {
                let _ = events
                    .send(RagEvent::ReferencesFailed {
                        id,
                        message: "a reference extraction is already running".to_owned(),
                    })
                    .await;
                return;
            }
            remember_citation_folder(&shared, &folder);
            let cancel = CancellationToken::new();
            shared
                .cancels
                .lock()
                .expect("cancel map")
                .insert(id, cancel.clone());
            tokio::spawn(run_references(shared, id, folder, options, cancel, events));
        }
        #[cfg(feature = "grobid")]
        RagCommand::StopReferences { id } => {
            if let Some(cancel) = shared.cancels.lock().expect("cancel map").remove(&id) {
                cancel.cancel();
            }
        }
        #[cfg(feature = "grobid")]
        RagCommand::RefreshReferences { folder } => {
            remember_citation_folder(&shared, &folder);
            let bibliography = bibliography_path(&shared);
            match references::load_entries(&bibliography, &folder) {
                Ok(entries) => {
                    let _ = events
                        .send(RagEvent::ReferencesLoaded {
                            entries,
                            bibliography,
                        })
                        .await;
                }
                Err(message) => {
                    let _ = events.send(RagEvent::Notice(message)).await;
                }
            }
        }
        RagCommand::Apply { setup } => {
            let current = {
                let guard = shared.session.lock().await;
                guard.as_ref().map(|session| session.session_id().clone())
            };
            for (_, cancel) in shared.cancels.lock().expect("cancel map").drain() {
                cancel.cancel();
            }
            rebuild_session(&shared, &events, Some(*setup)).await;
            announce_ready(&shared, &events).await;

            let loaded = match current {
                Some(id) => {
                    let mut guard = shared.session.lock().await;
                    match guard.as_mut() {
                        Some(session) => match session.load_session(&id).await {
                            Ok(true) => Some((id.0.clone(), session_turns(session))),
                            _ => None,
                        },
                        None => None,
                    }
                }
                None => None,
            };
            if let Some((id, turns)) = loaded {
                let _ = events.send(RagEvent::SessionLoaded { id, turns }).await;
            } else {
                let new_id = {
                    let guard = shared.session.lock().await;
                    guard.as_ref().map(|session| session.session_id().0.clone())
                };
                if let Some(id) = new_id {
                    let _ = events.send(RagEvent::SessionStarted { id }).await;
                }
            }
        }
    }
}

/// Build the agent from `setup`, replacing the current session.
///
/// When `setup` is `None` the current setup is reused. The session slot is
/// emptied first so the old store and generators are dropped before the new
/// ones open.
async fn rebuild_session(shared: &Arc<Shared>, events: &Sender<RagEvent>, setup: Option<RagSetup>) {
    let setup = setup.unwrap_or_else(|| shared.setup.lock().expect("setup").clone());
    trace(format!(
        "building agent in {}",
        setup.config.workspace.display()
    ));
    let mut guard = shared.session.lock().await;
    *guard = None;

    match build_session(&setup).await {
        Ok(session) => {
            trace("agent built");
            *guard = Some(session);
            *shared.setup.lock().expect("setup") = setup.clone();
            *shared.corpora.lock().expect("corpus map") = parse_corpora(&setup.config.corpus_dirs);
            drop(guard);
        }
        Err(message) => {
            trace(format!("agent build failed: {message}"));
            drop(guard);
            let _ = events.send(RagEvent::AgentFailed { message }).await;
        }
    }
}

async fn announce_ready(shared: &Arc<Shared>, events: &Sender<RagEvent>) {
    let info = agent_info(shared).await;
    if let Some(info) = info {
        trace(format!("announcing {}", info.chat_model));
        let _ = events.send(RagEvent::AgentReady(info)).await;
    }
    send_library(shared, events).await;
    send_sessions(shared, events).await;
}

async fn agent_info(shared: &Arc<Shared>) -> Option<AgentInfo> {
    let guard = shared.session.lock().await;
    let session = guard.as_ref()?;
    let agent = session.agent();
    let (workspace, pdf_backend) = {
        let setup = shared.setup.lock().expect("setup");
        (
            setup.config.workspace.clone(),
            pdf_backend_label(&setup.config.parse),
        )
    };
    let store = agent.store();
    Some(AgentInfo {
        workspace,
        chat_backend: agent.chat_agent().backend_name().to_owned(),
        chat_model: agent.chat_agent().model_name().to_owned(),
        embed_backend: agent.embedder().backend_name().to_owned(),
        embed_model: agent.embedder().model_name().to_owned(),
        hybrid: agent.embedder().is_enabled(),
        memory: format!("{:?}", session.memory_strategy()).to_lowercase(),
        pdf_backend,
        top_k: agent.top_k(),
        chunk_size: agent.chunk_config().size,
        chunk_overlap: agent.chunk_config().overlap,
        chunks: store.len(),
        documents: store.stored_documents().len(),
    })
}

fn session_turns(session: &AgentSession) -> Vec<SessionTurn> {
    session
        .turns()
        .iter()
        .map(|turn| SessionTurn {
            role: match turn.role {
                ragrig::TurnRole::User => crate::model::ChatRole::User,
                ragrig::TurnRole::Assistant => crate::model::ChatRole::Assistant,
                _ => crate::model::ChatRole::System,
            },
            text: turn.text.clone(),
        })
        .collect()
}

async fn send_sessions(shared: &Arc<Shared>, events: &Sender<RagEvent>) {
    let guard = shared.session.lock().await;
    let Some(session) = guard.as_ref() else {
        return;
    };
    match session.list_sessions().await {
        Ok(manifests) => {
            let sessions = manifests
                .into_iter()
                .map(|manifest| SessionInfo {
                    id: manifest.id.0,
                    turn_count: manifest.turn_count,
                    summary: manifest.summary,
                    updated: manifest.updated,
                })
                .collect();
            drop(guard);
            let _ = events.send(RagEvent::Sessions(sessions)).await;
        }
        Err(error) => {
            drop(guard);
            let _ = events
                .send(RagEvent::Notice(format!("cannot list sessions: {error:#}")))
                .await;
        }
    }
}

async fn send_library(shared: &Arc<Shared>, events: &Sender<RagEvent>) {
    let snapshot = library_snapshot(shared).await;
    let _ = events.send(RagEvent::Library(snapshot)).await;
}

async fn library_snapshot(shared: &Arc<Shared>) -> LibrarySnapshot {
    let guard = shared.session.lock().await;
    let Some(session) = guard.as_ref() else {
        return LibrarySnapshot::default();
    };
    let store = session.agent().store();
    let stored = store.stored_documents();
    let chunks = store.len();
    let corpora = shared.corpora.lock().expect("corpus map").clone();
    drop(guard);

    let mut scans: HashMap<String, HashMap<String, PathBuf>> = HashMap::new();
    let mut seen = HashSet::new();
    let mut documents = Vec::new();
    for document in stored {
        if !seen.insert((document.corpus.clone(), document.document.0.clone())) {
            continue;
        }
        let path = corpora.get(&document.corpus).and_then(|folder| {
            scans
                .entry(document.corpus.clone())
                .or_insert_with(|| scan_paths(folder))
                .get(&document.document.0)
                .cloned()
        });
        documents.push(LibraryDocument {
            corpus: document.corpus,
            name: document.document.0,
            parser: document.parser,
            chunker: document.chunker,
            path,
        });
    }
    documents.sort_by(|left, right| {
        left.corpus
            .cmp(&right.corpus)
            .then_with(|| left.name.cmp(&right.name))
    });
    LibrarySnapshot { documents, chunks }
}

fn scan_paths(folder: &Path) -> HashMap<String, PathBuf> {
    scan_document_files(folder)
        .into_iter()
        .map(|(document, name)| (name, document.path().clone()))
        .collect()
}

async fn run_ask(
    shared: Arc<Shared>,
    id: RequestId,
    prompt: String,
    retrieval: Option<String>,
    attachments: Vec<PendingAttachment>,
    cancel: CancellationToken,
    events: Sender<RagEvent>,
) {
    // Retrieval and generation share one query: the embedding model sees the
    // marked passage and the question together, and the model sees the same
    // text. The host transcript keeps echoing only `prompt`.
    let query = turn_query(&prompt, retrieval.as_deref());
    if events.send(RagEvent::Started { id }).await.is_err() {
        finish_ask(&shared, id);
        return;
    }

    #[cfg(feature = "grobid")]
    let citation_documents = {
        let CitationResolution {
            attachments,
            resolved,
            attached,
            missed,
        } = resolve_query_citations(&shared, &prompt).await;
        if !resolved.is_empty() || !missed.is_empty() {
            let _ = events
                .send(RagEvent::Citations {
                    resolved: attached,
                    missed,
                })
                .await;
        }
        attachments
    };
    #[cfg(not(feature = "grobid"))]
    let citation_documents: Vec<AttachedDocument> = Vec::new();

    if cancel.is_cancelled() {
        let _ = events
            .send(RagEvent::Stopped {
                id,
                partial: String::new(),
            })
            .await;
        finish_ask(&shared, id);
        return;
    }

    let parsers = {
        let setup = shared.setup.lock().expect("setup");
        DocumentParsers::new(filtered_parsers(&setup.config.parse))
    };
    let mut documents = citation_documents;
    documents.reserve(attachments.len());
    for attachment in &attachments {
        match resolve_attachment(&parsers, attachment) {
            Ok(document) => documents.push(document),
            Err(message) => {
                let _ = events.send(RagEvent::Failed { id, message }).await;
                finish_ask(&shared, id);
                return;
            }
        }
    }

    let pending = Arc::new(Mutex::new(String::new()));
    let collected = Arc::new(Mutex::new(String::new()));
    let streaming = shared.setup.lock().expect("setup").stream;
    let on_token = {
        let pending = Arc::clone(&pending);
        let collected = Arc::clone(&collected);
        move |token: String| {
            pending.lock().expect("token buffer").push_str(&token);
            collected.lock().expect("token buffer").push_str(&token);
        }
    };

    let (stop_tx, stop_rx) = async_channel::bounded(1);
    let batcher = spawn_batcher(id, Arc::clone(&pending), events.clone(), stop_rx);

    let (outcome, model) = {
        let mut guard = shared.session.lock().await;
        let Some(session) = guard.as_mut() else {
            drop(guard);
            let _ = stop_tx.send(()).await;
            let _ = batcher.await;
            let _ = events
                .send(RagEvent::Failed {
                    id,
                    message: "the agent is not ready".to_owned(),
                })
                .await;
            finish_ask(&shared, id);
            return;
        };
        let model = session.agent().chat_agent().model_name().to_owned();
        let outcome = if streaming {
            session
                .chat_streaming_detailed_with_attachments(
                    &query,
                    &documents,
                    &on_token,
                    Some(&cancel),
                )
                .await
        } else {
            session
                .chat_detailed_with_attachments(&query, &documents)
                .await
        };
        (outcome, model)
    };

    let _ = stop_tx.send(()).await;
    let _ = batcher.await;

    match outcome {
        Ok(response) => {
            let partial = collected.lock().expect("token buffer").clone();
            let reply = make_reply(response, model, partial);
            let _ = events.send(RagEvent::Finished { id, reply }).await;
            send_sessions(&shared, &events).await;
        }
        Err(error) => {
            let partial = collected.lock().expect("token buffer").clone();
            if error.downcast_ref::<Cancelled>().is_some() {
                let _ = events.send(RagEvent::Stopped { id, partial }).await;
            } else {
                let _ = events
                    .send(RagEvent::Failed {
                        id,
                        message: format!("{error:#}"),
                    })
                    .await;
            }
        }
    }

    finish_ask(&shared, id);
}

/// The query a turn runs on: the enrichment, when present, precedes the
/// question so a marked passage shapes retrieval.
fn turn_query(prompt: &str, retrieval: Option<&str>) -> String {
    match retrieval.map(str::trim).filter(|text| !text.is_empty()) {
        Some(retrieval) => format!("{retrieval}\n\n{prompt}"),
        None => prompt.to_owned(),
    }
}

fn finish_ask(shared: &Arc<Shared>, id: RequestId) {
    shared.generating.store(false, Ordering::Release);
    shared.cancels.lock().expect("cancel map").remove(&id);
}

fn spawn_batcher(
    id: RequestId,
    pending: Arc<Mutex<String>>,
    events: Sender<RagEvent>,
    stop: Receiver<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(TOKEN_BATCH);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let delta = {
                let mut buffer = pending.lock().expect("token buffer");
                std::mem::take(&mut *buffer)
            };
            if !delta.is_empty() && events.send(RagEvent::Chunk { id, delta }).await.is_err() {
                return;
            }
            if stop.try_recv().is_ok() {
                let delta = {
                    let mut buffer = pending.lock().expect("token buffer");
                    std::mem::take(&mut *buffer)
                };
                if !delta.is_empty() {
                    let _ = events.send(RagEvent::Chunk { id, delta }).await;
                }
                return;
            }
        }
    })
}

fn make_reply(response: RagResponse, model: String, partial: String) -> RagReply {
    let sources = sources(&response);
    let answer = if response.answer.is_empty() {
        partial
    } else {
        response.answer
    };
    RagReply {
        answer,
        model,
        chunks: response.chunks_retrieved,
        sources,
        elapsed: response.elapsed,
        rewritten_query: response.rewritten_query,
    }
}

fn sources(response: &RagResponse) -> Vec<SourceRef> {
    let Some(retrieved) = &response.retrieved else {
        return Vec::new();
    };
    let mut seen = HashSet::new();
    let mut sources = Vec::new();
    for scored in retrieved {
        let document = scored.chunk.document.0.clone();
        if !seen.insert(document.clone()) {
            continue;
        }
        let location = scored
            .chunk
            .meta
            .page_number
            .map(|page| format!("p. {page}"))
            .or_else(|| scored.chunk.meta.section.clone());
        let mut source =
            SourceRef::new(document.clone(), document).score(scored.score.value as f32);
        if let Some(location) = location {
            source = source.location(location);
        }
        sources.push(source);
    }
    sources
}

async fn run_sync(
    shared: Arc<Shared>,
    id: RequestId,
    name: String,
    folder: PathBuf,
    cancel: CancellationToken,
    events: Sender<RagEvent>,
) {
    if !shared.setup.lock().expect("setup").hybrid {
        let _ = events
            .send(RagEvent::SyncFailed {
                id,
                message: "hybrid retrieval is off; enable it before indexing".to_owned(),
            })
            .await;
        finish_sync(&shared, id);
        return;
    }
    if !folder.is_dir() {
        let _ = events
            .send(RagEvent::SyncFailed {
                id,
                message: format!("'{}' is not a folder", folder.display()),
            })
            .await;
        finish_sync(&shared, id);
        return;
    }

    let _ = events
        .send(RagEvent::SyncStarted {
            id,
            corpus: name.clone(),
        })
        .await;

    let progress = Arc::new(Mutex::new(IngestProgress {
        corpus: name.clone(),
        ..IngestProgress::default()
    }));
    let sink = {
        let progress = Arc::clone(&progress);
        let events = events.clone();
        move |event: &ProgressEvent| {
            let snapshot = {
                let mut state = progress.lock().expect("ingest progress");
                apply_progress(&mut state, event);
                state.clone()
            };
            match events.try_send(RagEvent::SyncProgress {
                id,
                progress: snapshot,
            }) {
                Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Closed(_)) => {}
            }
        }
    };

    let corpus = FolderCorpus::named(name.clone(), folder);
    let result = {
        let guard = shared.session.lock().await;
        let Some(session) = guard.as_ref() else {
            drop(guard);
            let _ = events
                .send(RagEvent::SyncFailed {
                    id,
                    message: "the agent is not ready".to_owned(),
                })
                .await;
            finish_sync(&shared, id);
            return;
        };
        // Hold the session lock for the whole sync. Some store backends
        // deep-copy on clone, so ingesting through a cloned agent would
        // leave the session's store unchanged.
        session
            .agent()
            .sync_corpus_with_progress(&corpus, Some(&sink), Some(&cancel))
            .await
    };

    match result {
        Ok(results) => {
            let summary = SyncSummary {
                corpus: name,
                documents: results.iter().filter(|result| result.ok).count(),
                chunks: results.iter().map(|result| result.chunks).sum(),
                failed: results.iter().filter(|result| !result.ok).count(),
            };
            let _ = events.send(RagEvent::SyncFinished { id, summary }).await;
            send_library(&shared, &events).await;
        }
        Err(error) => {
            if error.downcast_ref::<Cancelled>().is_some() {
                let _ = events.send(RagEvent::SyncStopped { id }).await;
            } else {
                let _ = events
                    .send(RagEvent::SyncFailed {
                        id,
                        message: format!("{error:#}"),
                    })
                    .await;
            }
        }
    }

    finish_sync(&shared, id);
}

fn finish_sync(shared: &Arc<Shared>, id: RequestId) {
    shared.syncing.store(false, Ordering::Release);
    shared.cancels.lock().expect("cancel map").remove(&id);
}

#[cfg(feature = "grobid")]
fn remember_citation_folder(shared: &Shared, folder: &Path) {
    // A folder parsed for references is citable even when it was not indexed
    // into the vector store. The synthetic name never matches a stored corpus,
    // so the library listing is unaffected.
    shared.corpora.lock().expect("corpus map").insert(
        format!("references:{}", folder.display()),
        folder.to_path_buf(),
    );
}

#[cfg(feature = "grobid")]
async fn resolve_query_citations(shared: &Arc<Shared>, prompt: &str) -> CitationResolution {
    let keys = grobid_bibtex::cite::cite_keys(prompt);
    if keys.is_empty() {
        return CitationResolution::default();
    }
    let setup = {
        let setup = shared.setup.lock().expect("setup");
        if !setup.resolve_citations {
            return CitationResolution::default();
        }
        setup.clone()
    };
    let bibliography = bibliography_path(shared);
    let shared = Arc::clone(shared);
    match tokio::task::spawn_blocking(move || {
        let corpora: Vec<PathBuf> = shared
            .corpora
            .lock()
            .expect("corpus map")
            .values()
            .cloned()
            .collect();
        let parsers = DocumentParsers::new(filtered_parsers(&setup.config.parse));
        let mut state = shared.citations.lock().expect("citation state");
        references::resolve_citations(&mut state, &bibliography, &corpora, &keys, &parsers)
    })
    .await
    {
        Ok(resolution) => resolution,
        Err(error) => {
            trace(format!("citation resolution failed: {error}"));
            CitationResolution {
                missed: vec![format!("@key citations could not be resolved: {error}")],
                ..CitationResolution::default()
            }
        }
    }
}

#[cfg(feature = "grobid")]
fn bibliography_path(shared: &Shared) -> PathBuf {
    let setup = shared.setup.lock().expect("setup");
    setup
        .bibliography
        .clone()
        .unwrap_or_else(|| setup.config.workspace.join(".ragrig/references.bib"))
}

#[cfg(feature = "grobid")]
async fn run_references(
    shared: Arc<Shared>,
    id: RequestId,
    folder: PathBuf,
    options: ReferenceOptions,
    cancel: CancellationToken,
    events: Sender<RagEvent>,
) {
    let bibliography = bibliography_path(&shared);
    let _ = events
        .send(RagEvent::ReferencesStarted {
            id,
            folder: folder.clone(),
        })
        .await;

    let progress_events = events.clone();
    let mut report_progress = move |progress: ReferenceProgress| {
        let _ = progress_events.try_send(RagEvent::ReferencesProgress { id, progress });
    };
    let is_cancelled = || cancel.is_cancelled();
    let mut observer = ExtractionObserver {
        progress: Some(&mut report_progress),
        cancel: Some(&is_cancelled),
    };

    match references::extract(&folder, &bibliography, &options, &mut observer).await {
        Ok((entries, report)) => {
            let _ = events
                .send(RagEvent::ReferencesLoaded {
                    entries,
                    bibliography,
                })
                .await;
            let _ = events
                .send(RagEvent::ReferencesFinished { id, report })
                .await;
        }
        Err(ExtractionError::Cancelled) => {
            let _ = events.send(RagEvent::ReferencesStopped { id }).await;
        }
        Err(ExtractionError::Failed(message)) => {
            let _ = events
                .send(RagEvent::ReferencesFailed { id, message })
                .await;
        }
    }

    finish_references(&shared, id);
}

#[cfg(feature = "grobid")]
fn finish_references(shared: &Arc<Shared>, id: RequestId) {
    shared.referencing.store(false, Ordering::Release);
    shared.cancels.lock().expect("cancel map").remove(&id);
}

fn apply_progress(progress: &mut IngestProgress, event: &ProgressEvent) {
    match event {
        ProgressEvent::FileStarted {
            index,
            total,
            corpus,
            document,
        } => {
            progress.corpus.clone_from(corpus);
            progress.document = Some(document.0.clone());
            progress.index = *index;
            progress.total = *total;
            progress.chunks = None;
            progress.phase = IngestPhase::Starting;
            progress.failed = None;
        }
        ProgressEvent::FileChunked { chunks } => {
            progress.chunks = Some(*chunks);
            progress.phase = IngestPhase::Parsed;
        }
        ProgressEvent::FileEmbedded { chunks } => {
            progress.embedded = progress.embedded.saturating_add(*chunks);
            progress.phase = IngestPhase::Embedding;
        }
        ProgressEvent::FileStored => {
            progress.phase = IngestPhase::Stored;
        }
        ProgressEvent::ChunksEmbedded { done, total } => {
            progress.embedded = *done;
            progress.total_chunks = *total;
            progress.phase = IngestPhase::Embedding;
        }
        ProgressEvent::FileFailed { error } => {
            progress.failed = Some(error.clone());
            progress.phase = IngestPhase::Failed;
        }
        _ => {}
    }
}

fn resolve_attachment(
    parsers: &DocumentParsers,
    attachment: &PendingAttachment,
) -> Result<AttachedDocument, String> {
    let content = match &attachment.source {
        AttachmentSource::Text(text) => text.clone(),
        AttachmentSource::File(path) => {
            let extension = path
                .extension()
                .and_then(|extension| extension.to_str())
                .unwrap_or("");
            if parsers.primary_name_for(extension).is_some() {
                extract_text(parsers, path).map_err(|error| {
                    format!("cannot parse attachment '{}': {error:#}", path.display())
                })?
            } else {
                fs::read_to_string(path).map_err(|error| {
                    format!("cannot read attachment '{}': {error}", path.display())
                })?
            }
        }
    };
    Ok(AttachedDocument {
        name: attachment.label.clone(),
        content,
    })
}

/// Apply a host prompt after the agent is built.
///
/// `RagAgent::builder().system_prompt` trusts the caller to include
/// `{context}`. A plain prompt without the placeholder silently drops both
/// retrieval and attachments, because Ragrig then has nowhere to substitute
/// the context block. `RagAgent::set_system_prompt` appends the placeholder
/// when it is missing, so every host prompt receives context.
fn apply_system_prompt(agent: &mut RagAgent, prompt: Option<&str>) {
    if let Some(prompt) = prompt.filter(|prompt| !prompt.trim().is_empty()) {
        agent.set_system_prompt(prompt.to_owned());
    }
}

async fn build_session(setup: &RagSetup) -> Result<AgentSession, String> {
    let mut config = setup.config.clone();
    let data_dir = setup
        .data_dir
        .clone()
        .unwrap_or_else(|| config.workspace.join(".ragrig"));
    if matches!(config.chat.provider, Provider::Deepseek) && config.chat.deepseek_api_key.is_none()
    {
        match std::env::var("DEEPSEEK_API_KEY") {
            Ok(key) if !key.trim().is_empty() => {
                config.chat.deepseek_api_key = Some(key);
            }
            _ => {
                return Err(
                    "DEEPSEEK_API_KEY is not set; export it before using DeepSeek".to_owned(),
                );
            }
        }
    }

    fs::create_dir_all(&config.workspace).map_err(|error| {
        format!(
            "cannot create workspace '{}': {error}",
            config.workspace.display()
        )
    })?;

    let chat_spec = ChatAgentSpec::try_from(&config.chat)
        .map_err(|error| format!("cannot configure the chat backend: {error:#}"))?;
    let chat = chat_spec
        .build()
        .map_err(|error| format!("cannot build the chat backend: {error:#}"))?;
    let embed_spec = if setup.hybrid {
        EmbedderSpec::try_from(&config.embed)
            .map_err(|error| format!("cannot configure the embedder: {error:#}"))?
    } else {
        // Off means no embedding model is built at all. Ragrig then skips
        // retrieval and the agent answers from explicit attachments.
        EmbedderSpec::none()
    };
    let embedder = embed_spec
        .build()
        .map_err(|error| format!("cannot build the embedder: {error:#}"))?;
    let vector_store = store::open_store(&data_dir)
        .await
        .map_err(|error| format!("cannot open the vector store: {error:#}"))?;
    let parsers = DocumentParsers::new(filtered_parsers(&config.parse));
    let chunk_config = ChunkConfig::new(config.parse.chunk_size, config.parse.chunk_overlap)
        .map_err(|error| format!("invalid chunk configuration: {error:#}"))?;

    let memory_agent = match config.memory.strategy {
        MemoryStrategyKind::Off | MemoryStrategyKind::Transcript => None,
        _ => Some(
            ChatAgentSpec::ollama(
                config.memory.model.clone(),
                config.chat.params.clone(),
                config.chat.request_timeout_secs,
            )
            .build()
            .map_err(|error| format!("cannot build the memory model: {error:#}"))?,
        ),
    };

    let mut builder = RagAgent::builder()
        .chat(chat)
        .embed(embedder)
        .store(vector_store)
        .parsers(parsers)
        .chunk_config(chunk_config)
        .attach_strategy(Box::new(ScicoAttach::with_context_tokens(
            config.chat.context_tokens,
        )))
        .context_tokens(config.chat.context_tokens)
        .top_k(config.embed.top_k)
        .similarity_threshold(config.embed.similarity_threshold);
    if let Some(agent) = memory_agent {
        builder = builder.rewriter(agent);
    }
    let system_prompt = match &setup.system_prompt {
        // An explicitly empty override suppresses the prompt file, matching
        // the previous builder behavior.
        Some(prompt) if prompt.trim().is_empty() => None,
        Some(prompt) => Some(prompt.clone()),
        None => match &config.chat.system_prompt_path {
            Some(path) => Some(fs::read_to_string(path).map_err(|error| {
                format!("cannot read system prompt '{}': {error}", path.display())
            })?),
            None => None,
        },
    };

    let mut agent = builder
        .build()
        .map_err(|error| format!("cannot build the RAG agent: {error:#}"))?;
    apply_system_prompt(&mut agent, system_prompt.as_deref());

    let sessions = FsSessionStore::new(data_dir.join("sessions"))
        .map_err(|error| format!("cannot open the session store: {error:#}"))?;
    let mut session = AgentSession::new(agent, Box::new(sessions));

    match config.memory.strategy {
        MemoryStrategyKind::Log => {
            session.set_history_strategy(Some(Box::new(LogHistory)));
        }
        MemoryStrategyKind::Summary => {
            let summary = ChatAgentSpec::ollama(
                config.memory.model.clone(),
                ragrig::GenerationParams::default(),
                config.chat.request_timeout_secs,
            )
            .build()
            .map_err(|error| format!("cannot build the summary model: {error:#}"))?;
            session.set_history_strategy(Some(Box::new(SummaryHistory::new(summary))));
        }
        MemoryStrategyKind::Off => {
            session.set_use_transcript(false);
            session.clear_turns();
        }
        _ => {}
    }

    Ok(session)
}

/// Keep the selected PDF backend plus an optional sloppy fallback.
fn filtered_parsers(config: &ParseConfig) -> Vec<Box<dyn ragrig::DocumentParser>> {
    let selected = match config.pdf_parser {
        ragrig::PdfParserBackend::Unpdf => "unpdf",
        ragrig::PdfParserBackend::Sink => "pdfsink",
        ragrig::PdfParserBackend::Internal => "sloppy-pdf",
        _ => "pdf-extract",
    };
    let fallback = "sloppy-pdf";
    build_parsers()
        .into_iter()
        .filter(|parser| {
            if !parser.extensions().contains(&"pdf") {
                return true;
            }
            parser.name() == selected
                || (config.sloppy_pdf && parser.name() == fallback && selected != fallback)
        })
        .collect()
}

fn pdf_backend_label(config: &ParseConfig) -> String {
    let label = match config.pdf_parser {
        ragrig::PdfParserBackend::Unpdf => "unpdf",
        ragrig::PdfParserBackend::Sink => "pdfsink",
        ragrig::PdfParserBackend::Internal => "sloppy-pdf",
        _ => "pdf-extract",
    };
    if config.sloppy_pdf && label != "sloppy-pdf" {
        format!("{label} + sloppy fallback")
    } else {
        label.to_owned()
    }
}

fn parse_corpora(entries: &[String]) -> HashMap<String, PathBuf> {
    entries
        .iter()
        .filter_map(|entry| entry.split_once('='))
        .map(|(name, path)| (name.trim().to_owned(), PathBuf::from(path.trim())))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::{fmt, sync::Arc, time::Duration};

    use anyhow::Result;
    use async_trait::async_trait;
    use ragrig::{
        AttachStrategy, Generator, RagrigConfig,
        embed::{Embedder, Embedding},
    };
    use tokio::time::timeout;

    use super::*;

    const FAKE_ANSWER: &str = "fake answer from the worker";

    #[derive(Clone)]
    struct FakeChat {
        chunks: Vec<String>,
        delay: Duration,
        prompts: Arc<Mutex<Vec<String>>>,
    }

    impl fmt::Debug for FakeChat {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.debug_struct("FakeChat").finish()
        }
    }

    #[async_trait]
    impl Generator for FakeChat {
        async fn generate_stream(
            &self,
            prompt: &str,
            on_token: &(dyn Fn(String) + Sync),
        ) -> Result<()> {
            self.generate_stream_with_cancel(prompt, on_token, &|| false)
                .await
        }

        async fn generate_stream_with_cancel(
            &self,
            prompt: &str,
            on_token: &(dyn Fn(String) + Sync),
            cancel: &dyn ragrig::progress::Cancel,
        ) -> Result<()> {
            self.prompts
                .lock()
                .expect("prompts")
                .push(prompt.to_owned());
            for chunk in &self.chunks {
                if cancel.is_cancelled() {
                    return Err(anyhow::Error::new(Cancelled));
                }
                if !self.delay.is_zero() {
                    tokio::time::sleep(self.delay).await;
                }
                on_token(chunk.clone());
            }
            Ok(())
        }

        fn backend_name(&self) -> &'static str {
            "fake"
        }

        fn model_name(&self) -> &str {
            "fake-chat"
        }
    }

    #[derive(Clone, Debug)]
    struct FakeEmbedder;

    #[async_trait]
    impl Embedder for FakeEmbedder {
        async fn embed(&self, texts: Vec<String>) -> Result<Vec<Embedding>> {
            Ok(texts
                .into_iter()
                .map(|text| Embedding {
                    text,
                    vector: vec![1.0, 0.0, 0.0, 0.0],
                })
                .collect())
        }

        fn backend_name(&self) -> &'static str {
            "fake"
        }

        fn model_name(&self) -> &str {
            "fake-embed"
        }

        fn dimension(&self) -> usize {
            4
        }
    }

    fn temp_workspace(label: &str) -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let counter = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "ragrig-gpui-actor-{label}-{}-{counter}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).expect("temp workspace");
        path
    }

    fn setup(workspace: &Path) -> RagSetup {
        let mut config = RagrigConfig {
            workspace: workspace.to_path_buf(),
            ..RagrigConfig::default()
        };
        config.chat.model = "fake-chat".to_owned();
        config.embed.model = "fake-embed".to_owned();
        config.parse.chunk_size = 64;
        config.parse.chunk_overlap = 8;
        config.memory.strategy = MemoryStrategyKind::Transcript;
        RagSetup::new(config)
    }

    async fn test_agent(
        workspace: &Path,
        chunks: Vec<String>,
        delay: Duration,
    ) -> (RagAgent, Arc<Mutex<Vec<String>>>) {
        let prompts = Arc::new(Mutex::new(Vec::new()));
        let chat = FakeChat {
            chunks,
            delay,
            prompts: Arc::clone(&prompts),
        };
        let store = store::open_store(workspace).await.expect("open store");
        let agent = RagAgent::builder()
            .chat(Box::new(chat))
            .embed(Box::new(FakeEmbedder))
            .store(store)
            .chunk_config(ChunkConfig::new(64, 8).expect("chunk config"))
            .attach_strategy(
                Box::new(ScicoAttach::with_context_tokens(4096)) as Box<dyn AttachStrategy>
            )
            .build()
            .expect("build agent");
        (agent, prompts)
    }

    async fn test_session(
        workspace: &Path,
        chunks: Vec<String>,
        delay: Duration,
    ) -> (AgentSession, Arc<Mutex<Vec<String>>>) {
        let (agent, prompts) = test_agent(workspace, chunks, delay).await;
        let sessions =
            FsSessionStore::new(workspace.join(".ragrig/sessions")).expect("session store");
        (AgentSession::new(agent, Box::new(sessions)), prompts)
    }

    fn spawn(
        workspace: &Path,
        session: AgentSession,
    ) -> (
        Sender<RagCommand>,
        Receiver<RagEvent>,
        thread::JoinHandle<()>,
    ) {
        spawn_with(setup(workspace), session)
    }

    fn spawn_with(
        setup: RagSetup,
        session: AgentSession,
    ) -> (
        Sender<RagCommand>,
        Receiver<RagEvent>,
        thread::JoinHandle<()>,
    ) {
        let (commands, command_rx) = async_channel::unbounded();
        let (events, event_rx) = async_channel::unbounded();
        let handle = start(setup, Some(session), command_rx, events).expect("start worker");
        (commands, event_rx, handle)
    }

    async fn next_event(events: &Receiver<RagEvent>) -> RagEvent {
        let event = timeout(Duration::from_secs(15), events.recv())
            .await
            .expect("event timeout")
            .expect("event channel");
        if std::env::var_os("RAGRIG_ACTOR_TRACE").is_some() {
            eprintln!("actor event: {event:?}");
        }
        event
    }

    async fn wait_for<F>(events: &Receiver<RagEvent>, predicate: F) -> RagEvent
    where
        F: Fn(&RagEvent) -> bool,
    {
        loop {
            let event = next_event(events).await;
            if predicate(&event) {
                return event;
            }
        }
    }

    fn answer_chunks() -> Vec<String> {
        FAKE_ANSWER
            .split_inclusive(char::is_whitespace)
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn retrieval_enrichment_precedes_the_question() {
        assert_eq!(turn_query("Why?", None), "Why?");
        assert_eq!(turn_query("Why?", Some("  \n")), "Why?");
        assert_eq!(
            turn_query("Why?", Some("MARKED PASSAGE\n")),
            "MARKED PASSAGE\n\nWhy?"
        );
    }

    #[tokio::test]
    async fn an_empty_library_still_streams_an_answer() {
        let workspace = temp_workspace("empty");
        let (session, _) = test_session(&workspace, answer_chunks(), Duration::ZERO).await;
        let (commands, events, handle) = spawn(&workspace, session);

        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;
        commands
            .send(RagCommand::Ask {
                id: RequestId::new(1),
                prompt: "hello".to_owned(),
                retrieval: None,
                attachments: Vec::new(),
            })
            .await
            .expect("send ask");

        let _ = wait_for(&events, |event| matches!(event, RagEvent::Started { .. })).await;
        let reply = loop {
            match next_event(&events).await {
                RagEvent::Finished { reply, .. } => break reply,
                RagEvent::Chunk { .. } | RagEvent::Sessions(_) | RagEvent::Library(_) => {}
                other => panic!("unexpected event: {other:?}"),
            }
        };
        assert_eq!(reply.answer, FAKE_ANSWER);
        assert_eq!(reply.model, "fake-chat");
        assert!(reply.chunks.is_none());

        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn a_marked_passage_enriches_the_retrieval_query() {
        let workspace = temp_workspace("enrich");
        let (session, prompts) = test_session(&workspace, answer_chunks(), Duration::ZERO).await;
        let (commands, events, handle) = spawn(&workspace, session);

        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;
        commands
            .send(RagCommand::Ask {
                id: RequestId::new(11),
                prompt: "find related work".to_owned(),
                retrieval: Some("A MARKED PASSAGE ABOUT CAUSALITY".to_owned()),
                attachments: Vec::new(),
            })
            .await
            .expect("send ask");

        let _ = wait_for(&events, |event| matches!(event, RagEvent::Started { .. })).await;
        loop {
            match next_event(&events).await {
                RagEvent::Finished { .. } => break,
                RagEvent::Chunk { .. } | RagEvent::Sessions(_) | RagEvent::Library(_) => {}
                other => panic!("unexpected event: {other:?}"),
            }
        }

        let prompts = prompts.lock().expect("prompts");
        assert_eq!(prompts.len(), 1);
        let passage = prompts[0]
            .find("A MARKED PASSAGE ABOUT CAUSALITY")
            .expect("the marked passage reaches the agent");
        let question = prompts[0]
            .find("find related work")
            .expect("the question reaches the agent");
        assert!(
            passage < question,
            "the passage must precede the question in the shared retrieval query"
        );

        drop(prompts);
        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn stopping_keeps_the_partial_answer_and_frees_the_session() {
        let workspace = temp_workspace("cancel");
        let chunks = vec!["word ".to_owned(); 200];
        let (session, _) = test_session(&workspace, chunks, Duration::from_millis(5)).await;
        let (commands, events, handle) = spawn(&workspace, session);
        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;

        commands
            .send(RagCommand::Ask {
                id: RequestId::new(7),
                prompt: "slow".to_owned(),
                retrieval: None,
                attachments: Vec::new(),
            })
            .await
            .expect("send ask");
        let _ = wait_for(&events, |event| matches!(event, RagEvent::Chunk { .. })).await;
        commands
            .send(RagCommand::Stop {
                id: RequestId::new(7),
            })
            .await
            .expect("send stop");

        let partial = wait_for(&events, |event| matches!(event, RagEvent::Stopped { .. })).await;
        let RagEvent::Stopped { partial, .. } = partial else {
            unreachable!()
        };
        assert!(!partial.is_empty());
        assert!(partial.len() < 200 * "word ".len());

        commands
            .send(RagCommand::NewSession)
            .await
            .expect("send new session");
        let started = wait_for(&events, |event| {
            matches!(event, RagEvent::SessionStarted { .. })
        })
        .await;
        assert!(matches!(started, RagEvent::SessionStarted { .. }));

        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn sessions_are_listed_loaded_and_reset() {
        let workspace = temp_workspace("sessions");
        let (session, _) = test_session(&workspace, answer_chunks(), Duration::ZERO).await;
        let (commands, events, handle) = spawn(&workspace, session);
        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;

        commands
            .send(RagCommand::Ask {
                id: RequestId::new(1),
                prompt: "remember this".to_owned(),
                retrieval: None,
                attachments: Vec::new(),
            })
            .await
            .expect("send ask");
        let _ = wait_for(&events, |event| matches!(event, RagEvent::Finished { .. })).await;

        commands
            .send(RagCommand::RefreshSessions)
            .await
            .expect("refresh");
        let sessions = wait_for(
            &events,
            |event| matches!(event, RagEvent::Sessions(list) if !list.is_empty()),
        )
        .await;
        let RagEvent::Sessions(sessions) = sessions else {
            unreachable!()
        };
        assert_eq!(sessions[0].turn_count, 2);
        let id = sessions[0].id.clone();

        commands
            .send(RagCommand::NewSession)
            .await
            .expect("new session");
        let _ = wait_for(&events, |event| {
            matches!(event, RagEvent::SessionStarted { .. })
        })
        .await;

        commands
            .send(RagCommand::LoadSession { id })
            .await
            .expect("load session");
        let loaded = wait_for(&events, |event| {
            matches!(event, RagEvent::SessionLoaded { .. })
        })
        .await;
        let RagEvent::SessionLoaded { turns, .. } = loaded else {
            unreachable!()
        };
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].text, "remember this");
        assert_eq!(turns[1].text, FAKE_ANSWER);

        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn indexing_a_folder_reports_documents_and_paths() {
        let workspace = temp_workspace("ingest");
        let corpus = workspace.join("papers");
        std::fs::create_dir_all(&corpus).expect("corpus folder");
        std::fs::write(
            corpus.join("note.md"),
            "# Note\n\nThe retrieval index keeps this paragraph.\n",
        )
        .expect("write note");

        let (session, _) = test_session(&workspace, answer_chunks(), Duration::ZERO).await;
        let (commands, events, handle) = spawn(&workspace, session);
        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;

        commands
            .send(RagCommand::SyncCorpus {
                id: RequestId::new(3),
                name: "papers".to_owned(),
                folder: corpus.clone(),
            })
            .await
            .expect("sync corpus");

        let summary = wait_for(&events, |event| {
            matches!(event, RagEvent::SyncFinished { .. })
        })
        .await;
        let RagEvent::SyncFinished { summary, .. } = summary else {
            unreachable!()
        };
        assert_eq!(summary.documents, 1);
        assert_eq!(summary.failed, 0);
        assert!(summary.chunks >= 1);

        let library = wait_for(
            &events,
            |event| matches!(event, RagEvent::Library(snapshot) if !snapshot.documents.is_empty()),
        )
        .await;
        let RagEvent::Library(snapshot) = library else {
            unreachable!()
        };
        assert_eq!(snapshot.documents[0].name, "note.md");
        assert_eq!(snapshot.documents[0].corpus, "papers");
        assert_eq!(
            snapshot.documents[0].path.as_deref(),
            Some(corpus.join("note.md").as_path())
        );
        assert!(snapshot.chunks >= 1);

        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn applying_configuration_reloads_the_saved_session() {
        let workspace = temp_workspace("apply");
        let (session, _) = test_session(&workspace, answer_chunks(), Duration::ZERO).await;
        let (commands, events, handle) = spawn(&workspace, session);
        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;

        commands
            .send(RagCommand::Ask {
                id: RequestId::new(1),
                prompt: "keep this turn".to_owned(),
                retrieval: None,
                attachments: Vec::new(),
            })
            .await
            .expect("send ask");
        let _ = wait_for(&events, |event| matches!(event, RagEvent::Finished { .. })).await;

        commands
            .send(RagCommand::Apply {
                setup: Box::new(setup(&workspace)),
            })
            .await
            .expect("apply settings");

        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;
        let loaded = wait_for(
            &events,
            |event| matches!(event, RagEvent::SessionLoaded { turns, .. } if !turns.is_empty()),
        )
        .await;
        let RagEvent::SessionLoaded { turns, .. } = loaded else {
            unreachable!()
        };
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].text, "keep this turn");
        assert_eq!(turns[1].text, FAKE_ANSWER);

        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn a_plain_system_prompt_still_receives_attachments() {
        let workspace = temp_workspace("prompt-context");
        let (mut agent, prompts) = test_agent(&workspace, answer_chunks(), Duration::ZERO).await;
        apply_system_prompt(&mut agent, Some("Answer tersely."));
        assert!(
            agent.system_prompt().contains("{context}"),
            "the agent must have a context placeholder"
        );

        let sessions =
            FsSessionStore::new(workspace.join(".ragrig/sessions")).expect("session store");
        let mut session = AgentSession::new(agent, Box::new(sessions));
        let result = session
            .chat_detailed_with_attachments(
                "summarise the paper",
                &[AttachedDocument {
                    name: "@Kano2025".to_owned(),
                    content: "ATTACHED PAPER BODY".to_owned(),
                }],
            )
            .await;
        assert!(result.is_ok());

        let prompts = prompts.lock().expect("prompts");
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("Answer tersely."));
        assert!(prompts[0].contains("<scico:v1:context source=\"creator\">"));
        assert!(prompts[0].contains(
            "<scico:v1:document name=\"@Kano2025\">ATTACHED PAPER BODY</scico:v1:document>"
        ));

        let _ = std::fs::remove_dir_all(workspace);
    }

    #[cfg(feature = "grobid")]
    #[tokio::test]
    async fn citations_attach_the_paper_and_reach_the_prompt() {
        let workspace = temp_workspace("cite");
        let corpus = workspace.join("papers");
        std::fs::create_dir_all(&corpus).expect("corpus");
        let paper = corpus.join("Kano2025.md");
        std::fs::write(&paper, "CITED PAPER BODY").expect("paper");
        let bibliography = workspace.join(".ragrig/references.bib");
        std::fs::create_dir_all(bibliography.parent().expect("parent")).expect("ragrig dir");
        std::fs::write(
            &bibliography,
            format!(
                "@article{{Kano2025,\n  author = {{Kano, Fumihiro}},\n  year = {{2025}},\n  title = {{Gaze}},\n  file = {{{}}}\n}}\n",
                paper.display()
            ),
        )
        .expect("bibliography");

        let (session, prompts) = test_session(&workspace, answer_chunks(), Duration::ZERO).await;
        let mut setup = setup(&workspace);
        setup.config.corpus_dirs = vec![format!("papers={}", corpus.display())];
        let (commands, events, handle) = spawn_with(setup, session);
        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;

        commands
            .send(RagCommand::Ask {
                id: RequestId::new(6),
                prompt: "summarise @Kano2025".to_owned(),
                retrieval: None,
                attachments: Vec::new(),
            })
            .await
            .expect("send ask");
        let event = wait_for(&events, |event| matches!(event, RagEvent::Citations { .. })).await;
        let RagEvent::Citations { resolved, missed } = event else {
            unreachable!()
        };
        assert!(missed.is_empty(), "{missed:?}");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].key, "Kano2025");
        assert_eq!(resolved[0].path, paper);

        let _ = wait_for(&events, |event| matches!(event, RagEvent::Finished { .. })).await;
        let prompts = prompts.lock().expect("prompts");
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("CITED PAPER BODY"));

        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn several_file_attachments_reach_the_prompt_as_documents() {
        let workspace = temp_workspace("attach-files");
        std::fs::create_dir_all(workspace.join("papers")).expect("papers");
        let first = workspace.join("papers/Kano2025.md");
        let second = workspace.join("papers/Kobayashi2011.md");
        std::fs::write(&first, "KANO PAPER BODY").expect("first paper");
        std::fs::write(&second, "KOBAYASHI PAPER BODY").expect("second paper");

        let (session, prompts) = test_session(&workspace, answer_chunks(), Duration::ZERO).await;
        let (commands, events, handle) = spawn(&workspace, session);
        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;

        commands
            .send(RagCommand::Ask {
                id: RequestId::new(7),
                prompt: "compare the two papers".to_owned(),
                retrieval: None,
                attachments: vec![
                    PendingAttachment::file("first", "@Kano2025", first),
                    PendingAttachment::file("second", "@Kobayashi2011", second),
                ],
            })
            .await
            .expect("send ask");
        let _ = wait_for(&events, |event| matches!(event, RagEvent::Finished { .. })).await;

        let prompts = prompts.lock().expect("prompts");
        assert_eq!(prompts.len(), 1);
        assert!(
            prompts[0].contains(
                "<scico:v1:document name=\"@Kano2025\">KANO PAPER BODY</scico:v1:document>"
            )
        );
        assert!(prompts[0].contains(
            "<scico:v1:document name=\"@Kobayashi2011\">KOBAYASHI PAPER BODY</scico:v1:document>"
        ));

        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }

    #[tokio::test]
    async fn text_attachments_reach_the_prompt() {
        let workspace = temp_workspace("attach");
        let (session, prompts) = test_session(&workspace, answer_chunks(), Duration::ZERO).await;
        let (commands, events, handle) = spawn(&workspace, session);
        let _ = wait_for(&events, |event| matches!(event, RagEvent::AgentReady(_))).await;

        commands
            .send(RagCommand::Ask {
                id: RequestId::new(5),
                prompt: "summarise the selection".to_owned(),
                retrieval: None,
                attachments: vec![PendingAttachment::text(
                    "selection",
                    "Selection",
                    "ATTACHED CONTEXT",
                )],
            })
            .await
            .expect("send ask");
        let _ = wait_for(&events, |event| matches!(event, RagEvent::Finished { .. })).await;

        let prompts = prompts.lock().expect("prompts");
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].contains("ATTACHED CONTEXT"));

        commands.close();
        handle.join().expect("worker thread");
        let _ = std::fs::remove_dir_all(workspace);
    }
}
