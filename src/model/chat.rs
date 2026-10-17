//! Chat transcript data and its reducer.

use std::time::Duration;

/// Stable identity of one message in a [`ChatState`].
///
/// Identifiers stay valid for the lifetime of the transcript. Loading a new
/// transcript assigns fresh identifiers; hosts should not persist them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MessageId(u64);

impl MessageId {
    /// The numeric value, useful for element IDs and logs.
    pub fn get(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for MessageId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

/// Who authored a message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChatRole {
    /// The human user.
    User,
    /// The assistant or model.
    Assistant,
    /// A system notice, rendered without a bubble.
    System,
}

impl ChatRole {
    /// Short lowercase name, e.g. for logs.
    pub fn label(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Assistant => "assistant",
            Self::System => "system",
        }
    }
}

/// Lifecycle of one message.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum MessageState {
    /// Fully received.
    #[default]
    Complete,
    /// Tokens are still arriving.
    Streaming,
    /// The host stopped generation; the partial body is kept.
    Stopped,
    /// Generation failed. The partial body, if any, is kept.
    Error {
        /// Human-readable failure shown beside the body.
        message: String,
    },
}

impl MessageState {
    /// Returns whether tokens may still be appended.
    pub fn is_streaming(&self) -> bool {
        matches!(self, Self::Streaming)
    }
}

/// One retrieved source shown under an assistant message.
#[derive(Clone, Debug, PartialEq)]
pub struct SourceRef {
    /// Stable source identifier, used to deduplicate repeated chunks.
    pub id: String,
    /// Short title shown on the chip.
    pub title: String,
    /// Optional location such as `p. 4` or `methods section`.
    pub location: Option<String>,
    /// Optional retrieval score.
    pub score: Option<f32>,
}

impl SourceRef {
    /// A source with a title and no location or score.
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            location: None,
            score: None,
        }
    }

    /// Set the location label.
    pub fn location(mut self, location: impl Into<String>) -> Self {
        self.location = Some(location.into());
        self
    }

    /// Set the retrieval score.
    pub fn score(mut self, score: f32) -> Self {
        self.score = Some(score);
        self
    }
}

/// Pipeline metadata shown in a message footer.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MessageMeta {
    /// Model that produced the answer.
    pub model: Option<String>,
    /// Number of retrieved chunks that entered the prompt.
    pub chunks: Option<usize>,
    /// Distinct sources among the retrieved chunks.
    pub sources: Vec<SourceRef>,
    /// Number of streamed tokens, when the host counts them.
    pub tokens: Option<usize>,
    /// Wall-clock generation time.
    pub elapsed: Option<Duration>,
}

impl MessageMeta {
    /// One-line summary, e.g. `deepseek-chat · 12 chunks · 3 sources · 421 tokens · 8.2 s`.
    ///
    /// Source titles are rendered separately as chips, so only their count is
    /// included here.
    pub fn summary(&self) -> String {
        let mut parts = Vec::new();
        if let Some(model) = &self.model {
            parts.push(model.clone());
        }
        if let Some(chunks) = self.chunks {
            parts.push(format!("{chunks} chunks"));
        }
        if !self.sources.is_empty() {
            let count = self.sources.len();
            parts.push(if count == 1 {
                "1 source".to_owned()
            } else {
                format!("{count} sources")
            });
        }
        if let Some(tokens) = self.tokens {
            parts.push(format!("{tokens} tokens"));
        }
        if let Some(elapsed) = self.elapsed {
            parts.push(format!("{:.1} s", elapsed.as_secs_f32()));
        }
        parts.join(" · ")
    }

    /// Returns whether the footer has anything to show.
    pub fn is_empty(&self) -> bool {
        self.model.is_none()
            && self.chunks.is_none()
            && self.sources.is_empty()
            && self.tokens.is_none()
            && self.elapsed.is_none()
    }

    /// Add a source unless one with the same [`SourceRef::id`] is present.
    ///
    /// Returns whether the source was added.
    pub fn add_source(&mut self, source: SourceRef) -> bool {
        if self.sources.iter().any(|existing| existing.id == source.id) {
            return false;
        }
        self.sources.push(source);
        true
    }
}

/// A message as stored in a [`ChatState`].
#[derive(Clone, Debug, PartialEq)]
pub struct ChatMessage {
    id: MessageId,
    role: ChatRole,
    text: String,
    state: MessageState,
    meta: Option<MessageMeta>,
}

impl ChatMessage {
    /// Stable message identity.
    pub fn id(&self) -> MessageId {
        self.id
    }

    /// Author of the message.
    pub fn role(&self) -> ChatRole {
        self.role
    }

    /// Message body in Markdown.
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Lifecycle state.
    pub fn state(&self) -> &MessageState {
        &self.state
    }

    /// Footer metadata, if any.
    pub fn meta(&self) -> Option<&MessageMeta> {
        self.meta.as_ref()
    }
}

/// Input for [`ChatState::push`] and [`ChatState::replace`].
#[derive(Clone, Debug, PartialEq)]
pub struct NewMessage {
    /// Author of the message.
    pub role: ChatRole,
    /// Message body in Markdown.
    pub text: String,
    /// Lifecycle state.
    pub state: MessageState,
    /// Optional footer metadata.
    pub meta: Option<MessageMeta>,
}

impl NewMessage {
    /// A complete user message.
    pub fn user(text: impl Into<String>) -> Self {
        Self::new(ChatRole::User, text)
    }

    /// A complete assistant message.
    pub fn assistant(text: impl Into<String>) -> Self {
        Self::new(ChatRole::Assistant, text)
    }

    /// A complete system notice.
    pub fn system(text: impl Into<String>) -> Self {
        Self::new(ChatRole::System, text)
    }

    /// A message with the given role and complete state.
    pub fn new(role: ChatRole, text: impl Into<String>) -> Self {
        Self {
            role,
            text: text.into(),
            state: MessageState::Complete,
            meta: None,
        }
    }

    /// Mark the message as streaming.
    pub fn streaming(mut self) -> Self {
        self.state = MessageState::Streaming;
        self
    }

    /// Override the lifecycle state.
    pub fn state(mut self, state: MessageState) -> Self {
        self.state = state;
        self
    }

    /// Attach footer metadata.
    pub fn meta(mut self, meta: MessageMeta) -> Self {
        self.meta = Some(meta);
        self
    }
}

/// The transcript reducer.
///
/// This is plain data: construct it in a host or wrap it in an entity. The
/// rules are deliberately small:
///
/// - only [`MessageState::Streaming`] messages accept [`append`](Self::append);
/// - [`finish`](Self::finish) only moves a streaming message to complete;
/// - [`stop`](Self::stop) keeps the partial body;
/// - [`fail`](Self::fail) keeps the partial body and records the reason.
#[derive(Debug, Default)]
pub struct ChatState {
    next_id: u64,
    messages: Vec<ChatMessage>,
}

impl ChatState {
    /// All messages in display order.
    pub fn messages(&self) -> &[ChatMessage] {
        &self.messages
    }

    /// Number of messages.
    pub fn len(&self) -> usize {
        self.messages.len()
    }

    /// Returns whether the transcript is empty.
    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Look up a message by identity.
    pub fn get(&self, id: MessageId) -> Option<&ChatMessage> {
        self.messages.iter().find(|message| message.id == id)
    }

    /// The most recent message.
    pub fn last(&self) -> Option<&ChatMessage> {
        self.messages.last()
    }

    /// The most recent assistant message.
    pub fn last_assistant(&self) -> Option<&ChatMessage> {
        self.messages
            .iter()
            .rev()
            .find(|message| message.role == ChatRole::Assistant)
    }

    /// The message currently streaming, if any.
    pub fn streaming_id(&self) -> Option<MessageId> {
        self.messages
            .iter()
            .rev()
            .find(|message| message.state.is_streaming())
            .map(|message| message.id)
    }

    /// Returns whether any message is still streaming.
    pub fn is_streaming(&self) -> bool {
        self.streaming_id().is_some()
    }

    /// Append a message and assign it a fresh identity.
    pub fn push(&mut self, message: NewMessage) -> MessageId {
        let id = MessageId(self.next_id);
        self.next_id += 1;
        self.messages.push(ChatMessage {
            id,
            role: message.role,
            text: message.text,
            state: message.state,
            meta: message.meta,
        });
        id
    }

    /// Insert a message immediately before `before`.
    ///
    /// Returns `None` when `before` is unknown; the transcript is unchanged in
    /// that case.
    pub fn insert_before(&mut self, before: MessageId, message: NewMessage) -> Option<MessageId> {
        let index = self
            .messages
            .iter()
            .position(|candidate| candidate.id == before)?;
        let id = MessageId(self.next_id);
        self.next_id += 1;
        self.messages.insert(
            index,
            ChatMessage {
                id,
                role: message.role,
                text: message.text,
                state: message.state,
                meta: message.meta,
            },
        );
        Some(id)
    }

    /// Append a complete user message.
    pub fn push_user(&mut self, text: impl Into<String>) -> MessageId {
        self.push(NewMessage::user(text))
    }

    /// Append a complete system notice.
    pub fn push_system(&mut self, text: impl Into<String>) -> MessageId {
        self.push(NewMessage::system(text))
    }

    /// Append an empty streaming assistant message.
    pub fn begin_assistant(&mut self) -> MessageId {
        self.push(NewMessage::assistant("").streaming())
    }

    /// Append `delta` to a streaming message.
    ///
    /// Returns `false` for unknown messages and for messages that are no
    /// longer streaming, so late tokens can be dropped safely.
    pub fn append(&mut self, id: MessageId, delta: &str) -> bool {
        let Some(message) = self.messages.iter_mut().find(|message| message.id == id) else {
            return false;
        };
        if !message.state.is_streaming() {
            return false;
        }
        message.text.push_str(delta);
        true
    }

    /// Mark a streaming message as complete.
    pub fn finish(&mut self, id: MessageId) -> bool {
        self.transition(id, &MessageState::Complete)
    }

    /// Mark a streaming message as stopped by the user.
    pub fn stop(&mut self, id: MessageId) -> bool {
        self.transition(id, &MessageState::Stopped)
    }

    /// Mark a message as failed, keeping any partial body.
    pub fn fail(&mut self, id: MessageId, error: impl Into<String>) -> bool {
        let Some(message) = self.messages.iter_mut().find(|message| message.id == id) else {
            return false;
        };
        message.state = MessageState::Error {
            message: error.into(),
        };
        true
    }

    /// Replace a message's footer metadata.
    pub fn set_meta(&mut self, id: MessageId, meta: MessageMeta) -> bool {
        let Some(message) = self.messages.iter_mut().find(|message| message.id == id) else {
            return false;
        };
        message.meta = Some(meta);
        true
    }

    /// Add one source to a message's footer, deduplicating by source id.
    ///
    /// Returns whether the source was added.
    pub fn add_source(&mut self, id: MessageId, source: SourceRef) -> bool {
        let Some(message) = self.messages.iter_mut().find(|message| message.id == id) else {
            return false;
        };
        message
            .meta
            .get_or_insert_with(MessageMeta::default)
            .add_source(source)
    }

    /// Remove every message. Identities continue from the previous run.
    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Remove one message, keeping the relative order of the rest.
    ///
    /// Returns whether the message existed.
    pub fn remove(&mut self, id: MessageId) -> bool {
        let before = self.messages.len();
        self.messages.retain(|message| message.id != id);
        self.messages.len() != before
    }

    /// Replace the transcript, assigning fresh identities.
    pub fn replace(&mut self, messages: impl IntoIterator<Item = NewMessage>) -> Vec<MessageId> {
        self.messages.clear();
        messages
            .into_iter()
            .map(|message| self.push(message))
            .collect()
    }

    fn transition(&mut self, id: MessageId, next: &MessageState) -> bool {
        let Some(message) = self.messages.iter_mut().find(|message| message.id == id) else {
            return false;
        };
        if !message.state.is_streaming() {
            return false;
        }
        message.state = next.clone();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_streamed_message_is_reduced_in_order() {
        let mut state = ChatState::default();
        let user = state.push_user("Explain the method");
        let assistant = state.begin_assistant();

        assert_eq!(state.len(), 2);
        assert_eq!(state.messages()[0].id(), user);
        assert_eq!(state.messages()[0].role(), ChatRole::User);
        assert!(state.append(assistant, "The "));
        assert!(state.append(assistant, "method."));
        assert!(state.finish(assistant));

        let message = state.get(assistant).expect("assistant message");
        assert_eq!(message.text(), "The method.");
        assert_eq!(message.state(), &MessageState::Complete);
        assert!(!state.is_streaming());
    }

    #[test]
    fn stopping_keeps_the_partial_body_and_rejects_late_tokens() {
        let mut state = ChatState::default();
        let assistant = state.begin_assistant();
        assert!(state.append(assistant, "Partial"));

        assert!(state.stop(assistant));
        assert!(!state.append(assistant, " late"));
        assert!(!state.finish(assistant));

        let message = state.get(assistant).expect("assistant message");
        assert_eq!(message.text(), "Partial");
        assert_eq!(message.state(), &MessageState::Stopped);
    }

    #[test]
    fn failing_keeps_the_partial_body_and_reason() {
        let mut state = ChatState::default();
        let assistant = state.begin_assistant();
        assert!(state.append(assistant, "Partial"));

        assert!(state.fail(assistant, "connection reset"));

        let message = state.get(assistant).expect("assistant message");
        assert_eq!(message.text(), "Partial");
        assert_eq!(
            message.state(),
            &MessageState::Error {
                message: "connection reset".to_owned()
            }
        );
    }

    #[test]
    fn metadata_and_sources_are_attached_and_deduplicated() {
        let mut state = ChatState::default();
        let assistant = state.begin_assistant();
        assert!(state.set_meta(
            assistant,
            MessageMeta {
                model: Some("deepseek-chat".to_owned()),
                chunks: Some(12),
                ..Default::default()
            }
        ));
        assert!(state.add_source(assistant, SourceRef::new("paper", "Paper")));
        assert!(!state.add_source(assistant, SourceRef::new("paper", "Paper again")));

        let message = state.get(assistant).expect("assistant message");
        let meta = message.meta().expect("meta");
        assert_eq!(meta.sources.len(), 1);
        assert_eq!(meta.summary(), "deepseek-chat · 12 chunks · 1 source");
    }

    #[test]
    fn removing_a_message_keeps_the_rest_in_order() {
        let mut state = ChatState::default();
        let first = state.push_user("first");
        let second = state.push_user("second");
        let third = state.push_user("third");

        assert!(state.remove(second));
        assert!(!state.remove(second));
        assert_eq!(
            state
                .messages()
                .iter()
                .map(ChatMessage::id)
                .collect::<Vec<_>>(),
            vec![first, third]
        );
    }

    #[test]
    fn inserting_before_keeps_the_transcript_order() {
        let mut state = ChatState::default();
        let user = state.push_user("question");
        let assistant = state.begin_assistant();

        let note = state
            .insert_before(assistant, NewMessage::system("Attached: @Kano2025"))
            .expect("insert before the assistant");
        assert!(note > assistant);
        assert_eq!(
            state
                .messages()
                .iter()
                .map(ChatMessage::role)
                .collect::<Vec<_>>(),
            vec![ChatRole::User, ChatRole::System, ChatRole::Assistant]
        );
        assert_eq!(state.messages()[0].id(), user);
        assert_eq!(state.messages()[2].id(), assistant);

        assert!(
            state
                .insert_before(MessageId(999), NewMessage::system("late"))
                .is_none()
        );
        assert_eq!(state.len(), 3);
    }

    #[test]
    fn replacing_a_transcript_assigns_fresh_identities() {
        let mut state = ChatState::default();
        let old = state.push_user("old");
        let ids = state.replace([NewMessage::user("new"), NewMessage::assistant("answer")]);

        assert_eq!(state.len(), 2);
        assert_eq!(state.messages()[0].text(), "new");
        assert!(ids.iter().all(|id| *id > old));
    }

    #[test]
    fn append_and_transitions_ignore_unknown_ids() {
        let mut state = ChatState::default();
        let id = MessageId(999);
        assert!(!state.append(id, "x"));
        assert!(!state.finish(id));
        assert!(!state.stop(id));
        assert!(!state.fail(id, "x"));
        assert!(!state.set_meta(id, MessageMeta::default()));
        assert!(!state.add_source(id, SourceRef::new("s", "S")));
    }

    #[test]
    fn a_finished_message_does_not_accept_more_tokens() {
        let mut state = ChatState::default();
        let assistant = state.begin_assistant();
        assert!(state.finish(assistant));
        assert!(!state.append(assistant, "late"));

        assert_eq!(state.get(assistant).expect("assistant").text(), "");
    }
}
