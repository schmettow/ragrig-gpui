//! The composite RAG chat surface: transcript, attachments, and composer.
//!
//! [`RagChat`] wires the M1 primitives to a [`RagController`]. Hosts that
//! want a custom layout can still take the individual entities through
//! [`chat`](Self::chat), [`composer`](Self::composer), and
//! [`controller`](Self::controller).

use std::path::PathBuf;

use gpui_kit::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Window, component::v_flex, div, prelude::FluentBuilder as _, px,
};

#[cfg(feature = "grobid")]
use crate::references::CitationAttachment;
use crate::{
    attachments::{AttachmentEvent, AttachmentRows},
    chat_view::{ChatIdentity, ChatView, ChatViewEvent, EmptyState},
    composer::{Composer, ComposerEvent},
    model::{Attachment, ChatRole, MessageId, MessageMeta, NewMessage, SourceRef},
};

use super::{
    PendingAttachment, RagController, RagEvent, RagReply, RequestId, SessionTurn, count_label,
};

/// What the composite chat surface reports to its host.
#[derive(Clone, Debug, PartialEq)]
pub enum RagChatEvent {
    /// A one-line status for the host's status bar.
    Status(SharedString),
    /// The user clicked a source chip.
    SourceClicked(SourceRef),
    /// A message body was copied.
    Copied(MessageId),
}

struct PendingTurn {
    request: RequestId,
    message: MessageId,
    tokens: usize,
}

/// A complete chat surface backed by a Ragrig controller.
///
/// Create it with [`RagChat::new`], embed it directly (it implements
/// [`Render`]), and drive sessions through [`controller`](Self::controller).
/// Attachments are one-shot: they are cleared after each question.
pub struct RagChat {
    controller: Entity<RagController>,
    chat: Entity<ChatView>,
    composer: Entity<Composer>,
    pending: Option<PendingTurn>,
    attachments: Vec<PendingAttachment>,
    attachment_counter: u64,
    streaming: bool,
}

impl RagChat {
    /// Create the transcript, composer, and event wiring.
    pub fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        controller: Entity<RagController>,
    ) -> Self {
        let chat = cx.new(ChatView::new);
        let composer = cx.new(|cx| Composer::new(window, cx));

        cx.subscribe(&composer, |this, _, event: &ComposerEvent, cx| {
            this.on_composer(event.clone(), cx);
        })
        .detach();
        cx.subscribe(&chat, |this, _, event: &ChatViewEvent, cx| {
            this.on_chat_event(event.clone(), cx);
        })
        .detach();
        cx.subscribe(&controller, |this, _, event: &RagEvent, cx| {
            this.on_rag_event(event.clone(), cx);
        })
        .detach();

        composer.update(cx, |composer, cx| composer.focus(window, cx));

        Self {
            controller,
            chat,
            composer,
            pending: None,
            attachments: Vec::new(),
            attachment_counter: 0,
            streaming: false,
        }
    }

    /// Replace the empty-state card shown before the first question.
    pub fn set_empty_state(&mut self, empty: EmptyState, cx: &mut Context<Self>) {
        self.chat
            .update(cx, |chat, cx| chat.set_empty_state(empty, cx));
    }

    /// Set how the assistant signs its messages.
    pub fn set_assistant(&mut self, identity: ChatIdentity, cx: &mut Context<Self>) {
        self.chat
            .update(cx, |chat, cx| chat.set_assistant(identity, cx));
    }

    /// The transcript.
    pub fn chat(&self) -> &Entity<ChatView> {
        &self.chat
    }

    /// The composer.
    pub fn composer(&self) -> &Entity<Composer> {
        &self.composer
    }

    /// The controller, for session and library actions.
    pub fn controller(&self) -> &Entity<RagController> {
        &self.controller
    }

    /// Whether an answer is currently streaming.
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// Context waiting for the next question.
    pub fn attachments(&self) -> &[PendingAttachment] {
        &self.attachments
    }

    /// Attach a file. The worker parses it before generating.
    pub fn attach_file(
        &mut self,
        name: impl Into<String>,
        path: impl Into<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        self.attachment_counter = self.attachment_counter.wrapping_add(1);
        let id = format!("file-{}", self.attachment_counter);
        self.attachments
            .push(PendingAttachment::file(id, name, path));
        cx.notify();
    }

    /// Attach in-memory text, e.g. an editor selection.
    pub fn attach_text(
        &mut self,
        label: impl Into<String>,
        text: impl Into<String>,
        cx: &mut Context<Self>,
    ) {
        self.attachment_counter = self.attachment_counter.wrapping_add(1);
        let id = format!("text-{}", self.attachment_counter);
        self.attachments
            .push(PendingAttachment::text(id, label, text));
        cx.notify();
    }

    /// Drop one attachment by id.
    pub fn detach(&mut self, id: &str, cx: &mut Context<Self>) {
        self.attachments.retain(|attachment| attachment.id != id);
        cx.notify();
    }

    /// Drop every pending attachment.
    pub fn clear_attachments(&mut self, cx: &mut Context<Self>) {
        self.attachments.clear();
        cx.notify();
    }

    /// Ask a question programmatically, e.g. from a command palette or an
    /// onboarding action.
    pub fn ask(&mut self, prompt: impl Into<String>, cx: &mut Context<Self>) {
        self.send_prompt(prompt.into(), true, cx);
    }

    /// Stop the running answer, keeping the partial text.
    pub fn stop(&mut self, cx: &mut Context<Self>) {
        if let Some(pending) = &self.pending {
            self.controller
                .update(cx, |controller, cx| controller.stop(pending.request, cx));
            cx.emit(RagChatEvent::Status("Stopping…".into()));
        }
    }

    fn send_prompt(&mut self, prompt: String, echo_user: bool, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        if echo_user {
            self.chat.update(cx, |chat, cx| {
                chat.push_user(prompt.clone(), cx);
            });
        }
        let message = self.chat.update(cx, |chat, cx| chat.begin_assistant(cx));
        let attachments = std::mem::take(&mut self.attachments);

        match self
            .controller
            .update(cx, |controller, cx| controller.ask(prompt, attachments, cx))
        {
            Ok(request) => {
                self.pending = Some(PendingTurn {
                    request,
                    message,
                    tokens: 0,
                });
                self.streaming = true;
                // The composer is switched to Stop when `Started` arrives.
                self.chat.update(cx, |chat, cx| chat.scroll_to_end(cx));
                cx.emit(RagChatEvent::Status("Generating…".into()));
            }
            Err(error) => {
                self.chat
                    .update(cx, |chat, cx| chat.fail(message, error.clone(), cx));
                cx.emit(RagChatEvent::Status(error.into()));
            }
        }
        cx.notify();
    }

    fn retry(&mut self, id: MessageId, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }
        let prompt = {
            let state = self.chat.read(cx).state();
            let state = state.read(cx);
            let messages = state.messages();
            match messages.iter().position(|message| message.id() == id) {
                Some(index) if index + 1 == messages.len() => messages[..index]
                    .iter()
                    .rev()
                    .find(|message| message.role() == ChatRole::User)
                    .map(|message| message.text().to_owned()),
                _ => None,
            }
        };
        let Some(prompt) = prompt else {
            cx.emit(RagChatEvent::Status(
                "Only the last failed answer can be retried.".into(),
            ));
            return;
        };
        self.chat.update(cx, |chat, cx| {
            chat.remove(id, cx);
        });
        self.send_prompt(prompt, false, cx);
    }

    fn on_composer(&mut self, event: ComposerEvent, cx: &mut Context<Self>) {
        match event {
            ComposerEvent::Submit(text) => self.send_prompt(text, true, cx),
            ComposerEvent::Stop => self.stop(cx),
        }
    }

    fn on_chat_event(&mut self, event: ChatViewEvent, cx: &mut Context<Self>) {
        match event {
            ChatViewEvent::Retry(id) => {
                // ChatView emits this while it is still being updated; defer
                // the mutation until the effect cycle has unwound.
                let this = cx.entity();
                cx.defer(move |cx| {
                    this.update(cx, |chat, cx| chat.retry(id, cx));
                });
            }
            ChatViewEvent::SourceClicked(source) => {
                cx.emit(RagChatEvent::SourceClicked(source));
            }
            ChatViewEvent::Copied(id) => {
                cx.emit(RagChatEvent::Copied(id));
            }
        }
    }

    fn on_rag_event(&mut self, event: RagEvent, cx: &mut Context<Self>) {
        match event {
            RagEvent::Started { id } => {
                if self.matches(id) {
                    self.streaming = true;
                    self.composer
                        .update(cx, |composer, cx| composer.set_streaming(true, cx));
                    cx.emit(RagChatEvent::Status("Generating…".into()));
                }
            }
            RagEvent::Chunk { id, delta } => {
                if let Some(pending) = &mut self.pending
                    && pending.request == id
                {
                    pending.tokens = pending
                        .tokens
                        .saturating_add(delta.split_whitespace().count());
                    self.chat
                        .update(cx, |chat, cx| chat.append(pending.message, &delta, cx));
                }
            }
            RagEvent::Finished { id, reply } => {
                if let Some(pending) = self.pending.take().filter(|pending| pending.request == id) {
                    if pending.tokens == 0 && !reply.answer.is_empty() {
                        self.chat.update(cx, |chat, cx| {
                            chat.append(pending.message, &reply.answer, cx);
                        });
                    }
                    let meta = reply_meta(&reply, pending.tokens);
                    self.chat.update(cx, |chat, cx| {
                        chat.set_meta(pending.message, meta, cx);
                        chat.finish(pending.message, cx);
                    });
                    self.finish_streaming(cx);
                    cx.emit(RagChatEvent::Status("Answer complete".into()));
                }
            }
            RagEvent::Stopped { id, .. } => {
                if let Some(pending) = self.pending.take().filter(|pending| pending.request == id) {
                    self.chat
                        .update(cx, |chat, cx| chat.stop(pending.message, cx));
                    self.finish_streaming(cx);
                    cx.emit(RagChatEvent::Status("Stopped".into()));
                }
            }
            RagEvent::Failed { id, message } => {
                if let Some(pending) = self.pending.take().filter(|pending| pending.request == id) {
                    self.chat.update(cx, |chat, cx| {
                        chat.fail(pending.message, message.clone(), cx);
                    });
                    self.finish_streaming(cx);
                    cx.emit(RagChatEvent::Status(message.into()));
                }
            }
            RagEvent::SessionStarted { .. } => {
                self.pending = None;
                self.streaming = false;
                self.attachments.clear();
                self.chat.update(cx, |chat, cx| chat.clear(cx));
                self.composer
                    .update(cx, |composer, cx| composer.set_streaming(false, cx));
            }
            RagEvent::SessionLoaded { turns, .. } => {
                self.pending = None;
                self.streaming = false;
                self.attachments.clear();
                self.chat
                    .update(cx, |chat, cx| chat.replace(loaded_messages(turns), cx));
                self.composer
                    .update(cx, |composer, cx| composer.set_streaming(false, cx));
            }
            RagEvent::AgentReady(info) => {
                cx.emit(RagChatEvent::Status(
                    format!("{} ready", info.chat_model).into(),
                ));
            }
            RagEvent::AgentFailed { message } => {
                cx.emit(RagChatEvent::Status(message.into()));
            }
            RagEvent::SyncFinished { summary, .. } => {
                cx.emit(RagChatEvent::Status(
                    format!(
                        "Indexed {} ({}) from {}",
                        count_label(summary.documents, "document", "documents"),
                        count_label(summary.chunks, "chunk", "chunks"),
                        summary.corpus
                    )
                    .into(),
                ));
            }
            RagEvent::SyncStopped { .. } => {
                cx.emit(RagChatEvent::Status("Indexing stopped".into()));
            }
            RagEvent::SyncFailed { message, .. } => {
                cx.emit(RagChatEvent::Status(message.into()));
            }
            RagEvent::Notice(message) => {
                cx.emit(RagChatEvent::Status(message.into()));
            }
            #[cfg(feature = "grobid")]
            RagEvent::Citations { resolved, missed } => {
                if let Some(note) = citation_note(&resolved, &missed) {
                    let before = self.pending.as_ref().map(|pending| pending.message);
                    self.chat.update(cx, |chat, cx| {
                        let inserted = before.is_some_and(|before| {
                            chat.push_system_before(before, note.clone(), cx)
                        });
                        if !inserted {
                            chat.push_system(note, cx);
                        }
                    });
                }
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesStarted { .. } | RagEvent::ReferencesLoaded { .. } => {}
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesProgress { progress, .. } => {
                cx.emit(RagChatEvent::Status(progress.summary().into()));
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesFinished { report, .. } => {
                cx.emit(RagChatEvent::Status(report.summary().into()));
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesStopped { .. } => {
                cx.emit(RagChatEvent::Status("Reference extraction stopped".into()));
            }
            #[cfg(feature = "grobid")]
            RagEvent::ReferencesFailed { message, .. } => {
                cx.emit(RagChatEvent::Status(message.into()));
            }
            RagEvent::Sessions(_)
            | RagEvent::Library(_)
            | RagEvent::SyncStarted { .. }
            | RagEvent::SyncProgress { .. } => {}
        }
        cx.notify();
    }

    fn matches(&self, request: RequestId) -> bool {
        self.pending
            .as_ref()
            .is_some_and(|pending| pending.request == request)
    }

    fn finish_streaming(&mut self, cx: &mut Context<Self>) {
        self.streaming = false;
        self.composer
            .update(cx, |composer, cx| composer.set_streaming(false, cx));
    }
}

impl Render for RagChat {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = if self.attachments.is_empty() {
            None
        } else {
            let chips = self
                .attachments
                .iter()
                .map(|attachment| {
                    Attachment::pending(attachment.id.as_str(), attachment.label.as_str())
                })
                .collect::<Vec<_>>();
            let this = cx.entity();
            Some(AttachmentRows::new(chips).on_event(move |event, _, cx| {
                if let AttachmentEvent::Dropped(id) = event {
                    this.update(cx, |chat, cx| chat.detach(&id, cx));
                }
            }))
        };

        v_flex()
            .size_full()
            .child(div().flex_1().min_h(px(0.)).child(self.chat.clone()))
            .when_some(rows, |this, rows| {
                this.child(div().px_3().py_2().child(rows))
            })
            .child(self.composer.clone())
    }
}

impl EventEmitter<RagChatEvent> for RagChat {}

#[cfg(feature = "grobid")]
fn citation_note(resolved: &[CitationAttachment], missed: &[String]) -> Option<String> {
    if resolved.is_empty() && missed.is_empty() {
        return None;
    }
    let mut lines = Vec::with_capacity(missed.len() + 1);
    if !resolved.is_empty() {
        let keys = resolved
            .iter()
            .map(|attachment| format!("@{}", attachment.key))
            .collect::<Vec<_>>()
            .join(", ");
        lines.push(format!("Attached: {keys}"));
    }
    lines.extend(missed.iter().cloned());
    Some(lines.join("\n"))
}

fn reply_meta(reply: &RagReply, streamed_tokens: usize) -> MessageMeta {
    let tokens = streamed_tokens.max(reply.answer.split_whitespace().count());
    MessageMeta {
        model: Some(reply.model.clone()),
        chunks: reply.chunks,
        sources: reply.sources.clone(),
        tokens: (tokens > 0).then_some(tokens),
        elapsed: reply.elapsed,
    }
}

fn loaded_messages(turns: Vec<SessionTurn>) -> Vec<NewMessage> {
    turns
        .into_iter()
        .map(|turn| match turn.role {
            ChatRole::User => NewMessage::user(turn.text),
            ChatRole::Assistant => NewMessage::assistant(turn.text),
            ChatRole::System => NewMessage::system(turn.text),
        })
        .collect()
}
