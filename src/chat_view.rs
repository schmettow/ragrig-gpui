//! The transcript view.

use std::{collections::HashMap, time::Duration};

use gpui_kit::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, ClipboardItem, Context,
    ElementId, Entity, EventEmitter, Hsla, IntoElement, ParentElement as _, Render, SharedString,
    Styled as _, WeakEntity, Window,
    assets::IconName,
    component::{
        ActiveTheme as _, Sizable as _, Theme,
        button::{Button, ButtonVariants as _},
        h_flex,
        message_scroller::{MessageScroller, MessageScrollerState},
        text::{TextView, TextViewState, TextViewStyle},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::{
    markdown::stabilize_markdown,
    model::{
        ChatMessage, ChatRole, ChatState, MessageId, MessageMeta, MessageState, NewMessage,
        SourceRef,
    },
};

/// The empty-transcript state.
#[derive(Clone, Debug)]
pub struct EmptyState {
    /// Large icon above the title.
    pub icon: IconName,
    /// Heading, e.g. `Ask your library`.
    pub title: SharedString,
    /// One or two lines explaining what to do.
    pub description: SharedString,
}

impl Default for EmptyState {
    fn default() -> Self {
        Self {
            icon: IconName::Sparkles,
            title: "Ask your library".into(),
            description: "Answers cite the documents you attach or index.".into(),
        }
    }
}

/// What the user did inside the transcript.
#[derive(Clone, Debug, PartialEq)]
pub enum ChatViewEvent {
    /// The user asked to retry a failed message.
    Retry(MessageId),
    /// The user clicked a source chip.
    SourceClicked(SourceRef),
    /// The message body was copied.
    Copied(MessageId),
}

#[derive(Default)]
struct MessageTexts {
    states: HashMap<MessageId, Entity<TextViewState>>,
}

impl MessageTexts {
    fn get(&self, id: MessageId) -> Option<Entity<TextViewState>> {
        self.states.get(&id).cloned()
    }

    fn insert(&mut self, id: MessageId, state: Entity<TextViewState>) {
        self.states.insert(id, state);
    }

    fn remove(&mut self, id: MessageId) {
        self.states.remove(&id);
    }

    fn clear(&mut self) {
        self.states.clear();
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.states.len()
    }
}

/// A streamed, Markdown transcript with metadata and source chips.
///
/// `ChatView` owns the transcript state and one [`TextViewState`] per message,
/// so tokens can be appended without rebuilding the whole document. It emits
/// [`ChatViewEvent`] for actions the host must handle.
pub struct ChatView {
    state: Entity<ChatState>,
    texts: Entity<MessageTexts>,
    scroller: Entity<MessageScrollerState>,
    empty: EmptyState,
}

impl ChatView {
    /// Create an empty transcript.
    pub fn new(cx: &mut Context<Self>) -> Self {
        let state = cx.new(|_| ChatState::default());
        let texts = cx.new(|_| MessageTexts::default());
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        Self {
            state,
            texts,
            scroller,
            empty: EmptyState::default(),
        }
    }

    /// Replace the empty state.
    pub fn empty_state(mut self, empty: EmptyState) -> Self {
        self.empty = empty;
        self
    }

    /// Replace the empty state and redraw.
    pub fn set_empty_state(&mut self, empty: EmptyState, cx: &mut Context<Self>) {
        self.empty = empty;
        cx.notify();
    }

    /// The underlying reducer, for hosts that need read access.
    pub fn state(&self) -> &Entity<ChatState> {
        &self.state
    }

    /// Number of messages.
    pub fn len(&self, cx: &App) -> usize {
        self.state.read(cx).len()
    }

    /// Returns whether the transcript is empty.
    pub fn is_empty(&self, cx: &App) -> bool {
        self.state.read(cx).is_empty()
    }

    /// Append a message and create its render state.
    pub fn push_message(&mut self, message: NewMessage, cx: &mut Context<Self>) -> MessageId {
        let text = message.text.clone();
        let streaming = message.state.is_streaming();
        let id = self.state.update(cx, |state, _| state.push(message));
        self.cache_text(id, &text, streaming, cx);
        self.append_row(cx);
        cx.notify();
        id
    }

    /// Append a user message.
    pub fn push_user(&mut self, text: impl Into<String>, cx: &mut Context<Self>) -> MessageId {
        self.push_message(NewMessage::user(text), cx)
    }

    /// Append a system notice.
    pub fn push_system(&mut self, text: impl Into<String>, cx: &mut Context<Self>) -> MessageId {
        self.push_message(NewMessage::system(text), cx)
    }

    /// Insert a system notice immediately before `before`.
    ///
    /// Use this for per-question context notes: the note then appears between
    /// the question and the assistant placeholder, exactly where the attached
    /// papers belong. Returns `false` when `before` is unknown.
    pub fn push_system_before(
        &mut self,
        before: MessageId,
        text: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        let text = text.into();
        let Some(index) = self
            .state
            .read(cx)
            .messages()
            .iter()
            .position(|message| message.id() == before)
        else {
            return false;
        };
        let inserted = self.state.update(cx, |state, _| {
            state.insert_before(before, NewMessage::system(text.clone()))
        });
        let Some(id) = inserted else {
            return false;
        };
        self.cache_text(id, &text, false, cx);
        let spliced = self
            .scroller
            .update(cx, |scroller, cx| scroller.splice(index..index, 1, cx));
        debug_assert!(spliced, "the transcript and scroller counts diverged");
        cx.notify();
        true
    }

    /// Start an empty streaming assistant message.
    pub fn begin_assistant(&mut self, cx: &mut Context<Self>) -> MessageId {
        self.push_message(NewMessage::assistant("").streaming(), cx)
    }

    /// Append a token batch to a streaming message.
    pub fn append(&mut self, id: MessageId, delta: &str, cx: &mut Context<Self>) -> bool {
        if !self.state.update(cx, |state, _| state.append(id, delta)) {
            return false;
        }
        if let Some(text) = self.texts.read(cx).get(id) {
            text.update(cx, |text, cx| text.push_str(delta, cx));
        }
        self.remeasure_last(cx);
        cx.notify();
        true
    }

    /// Finish a streaming message and normalize its final Markdown.
    pub fn finish(&mut self, id: MessageId, cx: &mut Context<Self>) -> bool {
        if !self.state.update(cx, |state, _| state.finish(id)) {
            return false;
        }
        self.sync_text(id, cx);
        self.remeasure_last(cx);
        cx.notify();
        true
    }

    /// Stop a streaming message, keeping the partial body.
    pub fn stop(&mut self, id: MessageId, cx: &mut Context<Self>) -> bool {
        if !self.state.update(cx, |state, _| state.stop(id)) {
            return false;
        }
        self.sync_text(id, cx);
        self.remeasure_last(cx);
        cx.notify();
        true
    }

    /// Mark a message as failed, keeping the partial body.
    pub fn fail(
        &mut self,
        id: MessageId,
        error: impl Into<String>,
        cx: &mut Context<Self>,
    ) -> bool {
        if !self.state.update(cx, |state, _| state.fail(id, error)) {
            return false;
        }
        self.sync_text(id, cx);
        self.remeasure_last(cx);
        cx.notify();
        true
    }

    /// Replace a message's footer metadata.
    pub fn set_meta(&mut self, id: MessageId, meta: MessageMeta, cx: &mut Context<Self>) -> bool {
        if !self.state.update(cx, |state, _| state.set_meta(id, meta)) {
            return false;
        }
        self.remeasure_message(id, cx);
        cx.notify();
        true
    }

    /// Add one source to a message footer.
    pub fn add_source(&mut self, id: MessageId, source: SourceRef, cx: &mut Context<Self>) -> bool {
        if !self
            .state
            .update(cx, |state, _| state.add_source(id, source))
        {
            return false;
        }
        self.remeasure_message(id, cx);
        cx.notify();
        true
    }

    /// Remove every message from the transcript.
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, _| state.clear());
        self.texts.update(cx, |texts, _| texts.clear());
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(0, cx));
        cx.notify();
    }

    /// Remove one message, e.g. before retrying a failed answer.
    ///
    /// Returns whether the message existed.
    pub fn remove(&mut self, id: MessageId, cx: &mut Context<Self>) -> bool {
        if !self.state.update(cx, |state, _| state.remove(id)) {
            return false;
        }
        self.texts.update(cx, |texts, _| texts.remove(id));
        let len = self.state.read(cx).len();
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(len, cx));
        cx.notify();
        true
    }

    /// Replace the transcript, for loading a session.
    pub fn replace(
        &mut self,
        messages: impl IntoIterator<Item = NewMessage>,
        cx: &mut Context<Self>,
    ) {
        let ids = self.state.update(cx, |state, _| state.replace(messages));
        let texts = {
            let state = self.state.read(cx);
            ids.iter()
                .filter_map(|id| {
                    state.get(*id).map(|message| {
                        (
                            *id,
                            message.text().to_owned(),
                            message.state().is_streaming(),
                        )
                    })
                })
                .collect::<Vec<_>>()
        };
        self.texts.update(cx, |texts, _| texts.clear());
        for (id, text, streaming) in texts {
            self.cache_text(id, &text, streaming, cx);
        }
        self.scroller
            .update(cx, |scroller, cx| scroller.reset(ids.len(), cx));
        cx.notify();
    }

    /// Scroll to the newest message and resume tail following.
    pub fn scroll_to_end(&mut self, cx: &mut Context<Self>) {
        self.scroller
            .update(cx, |scroller, cx| scroller.scroll_to_end(cx));
    }

    /// Copy a message body to the clipboard.
    pub fn copy_message(&mut self, id: MessageId, cx: &mut Context<Self>) {
        let text = self
            .state
            .read(cx)
            .get(id)
            .map(|message| message.text().to_owned());
        let Some(text) = text else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        cx.emit(ChatViewEvent::Copied(id));
    }

    /// Ask the host to retry a failed message.
    pub fn retry(&mut self, id: MessageId, cx: &mut Context<Self>) {
        cx.emit(ChatViewEvent::Retry(id));
    }

    /// Ask the host to act on a source.
    pub fn source_clicked(&mut self, source: SourceRef, cx: &mut Context<Self>) {
        cx.emit(ChatViewEvent::SourceClicked(source));
    }

    fn cache_text(&mut self, id: MessageId, text: &str, streaming: bool, cx: &mut Context<Self>) {
        // A streaming body must stay raw: appending after a synthetic closing
        // fence would put later tokens outside the code block.
        let display = if streaming {
            text.to_owned()
        } else {
            stabilize_markdown(text).into_owned()
        };
        let state = cx.new(|cx| TextViewState::markdown(&display, cx));
        self.texts.update(cx, |texts, _| texts.insert(id, state));
    }

    fn sync_text(&mut self, id: MessageId, cx: &mut Context<Self>) {
        let text = self
            .state
            .read(cx)
            .get(id)
            .map(|message| message.text().to_owned());
        let view = self.texts.read(cx).get(id);
        if let (Some(text), Some(view)) = (text, view) {
            let display = stabilize_markdown(&text).into_owned();
            view.update(cx, |view, cx| view.set_text(&display, cx));
        }
    }

    fn append_row(&mut self, cx: &mut Context<Self>) {
        let appended = self
            .scroller
            .update(cx, |scroller, cx| scroller.append(1, cx));
        debug_assert!(appended, "the transcript and scroller counts diverged");
    }

    fn remeasure_last(&mut self, cx: &mut Context<Self>) {
        let count = self.state.read(cx).len();
        if count == 0 {
            return;
        }
        self.scroller.update(cx, |scroller, cx| {
            scroller.remeasure_items(count - 1..count, cx);
        });
    }

    fn remeasure_message(&mut self, id: MessageId, cx: &mut Context<Self>) {
        let index = self
            .state
            .read(cx)
            .messages()
            .iter()
            .position(|message| message.id() == id);
        if let Some(index) = index {
            self.scroller.update(cx, |scroller, cx| {
                scroller.remeasure_items(index..index + 1, cx);
            });
        }
    }
}

impl Render for ChatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let count = self.state.read(cx).len();
        if count == 0 {
            return empty_view(&self.empty, cx.theme());
        }

        let this = cx.entity().downgrade();
        let view_id = cx.entity_id().as_u64();
        let state = self.state.clone();
        let texts = self.texts.clone();

        MessageScroller::new(
            ElementId::View(cx.entity_id()),
            self.scroller.clone(),
            move |index, _, cx| {
                let message = state.read(cx).messages().get(index).cloned();
                match message {
                    Some(message) => {
                        let text = texts.read(cx).get(message.id());
                        message_row(&message, text, &this, view_id, cx)
                    }
                    None => div().into_any_element(),
                }
            },
        )
        .scrollbar(true)
        .jump_button(true)
        .with_jump_button_label("Latest")
        .with_bottom_fade(cx.theme().background)
        .flex_1()
        .min_h(px(0.))
        .into_any_element()
    }
}

impl EventEmitter<ChatViewEvent> for ChatView {}

fn empty_view(empty: &EmptyState, theme: &Theme) -> AnyElement {
    v_flex()
        .size_full()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .child(div().text_2xl().child(empty.icon))
        .child(
            div()
                .text_lg()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(empty.title.clone()),
        )
        .child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child(empty.description.clone()),
        )
        .into_any_element()
}

fn message_row(
    message: &ChatMessage,
    text: Option<Entity<TextViewState>>,
    this: &WeakEntity<ChatView>,
    view_id: u64,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme();
    let style = TextViewStyle {
        is_dark: theme.is_dark(),
        ..Default::default()
    };
    let body = match text {
        Some(text) => TextView::new(&text)
            .style(style.clone())
            .selectable(true)
            .into_any_element(),
        None => TextView::markdown(
            ElementId::Name(format!("ragrig-message-{view_id}-{}", message.id()).into()),
            message.text().to_owned(),
        )
        .style(style)
        .selectable(true)
        .into_any_element(),
    };

    let streaming = message.state().is_streaming();
    let error = match message.state() {
        MessageState::Error { message } => Some(message.as_str()),
        _ => None,
    };

    match message.role() {
        ChatRole::System => h_flex()
            .w_full()
            .justify_center()
            .px_4()
            .py_2()
            .child(
                div()
                    .text_sm()
                    .text_color(theme.muted_foreground)
                    .child(message.text().to_owned()),
            )
            .into_any_element(),
        ChatRole::User => {
            let card = v_flex()
                .max_w(px(720.))
                .gap_2()
                .p_3()
                .bg(theme.secondary)
                .rounded(theme.radius)
                .child(role_header(
                    IconName::CircleUser,
                    "You",
                    theme.muted_foreground,
                    message.id(),
                    message.state(),
                    view_id,
                    theme,
                ))
                .child(body)
                .child(message_footer(message, this, view_id, theme));

            h_flex()
                .w_full()
                .justify_end()
                .px_4()
                .py_2()
                .child(card)
                .into_any_element()
        }
        ChatRole::Assistant => {
            let mut row = v_flex().w_full().gap_2().px_4().py_3().child(role_header(
                IconName::Bot,
                "Ragrig",
                theme.primary,
                message.id(),
                message.state(),
                view_id,
                theme,
            ));
            if message.text().is_empty() && streaming {
                row = row.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Thinking…"),
                );
            } else {
                row = row.child(body);
            }
            if let Some(error) = error {
                row = row.child(error_banner(message.id(), error, this, view_id, theme));
            }
            row.child(message_footer(message, this, view_id, theme))
                .into_any_element()
        }
    }
}

fn role_header(
    icon: IconName,
    label: &'static str,
    color: Hsla,
    id: MessageId,
    state: &MessageState,
    view_id: u64,
    theme: &Theme,
) -> AnyElement {
    let streaming = state.is_streaming();
    let stopped = matches!(state, MessageState::Stopped);
    h_flex()
        .items_center()
        .gap_2()
        .child(div().text_color(color).child(icon))
        .child(
            div()
                .text_sm()
                .font_weight(gpui_kit::FontWeight::SEMIBOLD)
                .child(label),
        )
        .when(streaming, |row| {
            row.child(streaming_dot(view_id, id, theme.primary)).child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("streaming"),
            )
        })
        .when(stopped, |row| row.child(tag("stopped", theme.warning)))
        .into_any_element()
}

fn message_footer(
    message: &ChatMessage,
    this: &WeakEntity<ChatView>,
    view_id: u64,
    theme: &Theme,
) -> AnyElement {
    let mut parts = h_flex().flex_wrap().items_center().gap_2();
    if let Some(meta) = message.meta() {
        let summary = meta.summary();
        if !summary.is_empty() {
            parts = parts.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(summary),
            );
        }
        for (index, source) in meta.sources.iter().enumerate() {
            parts = parts.child(source_chip(view_id, message.id(), index, source, this));
        }
    }

    let mut row = h_flex()
        .w_full()
        .justify_between()
        .items_end()
        .gap_2()
        .child(parts);
    if !message.text().is_empty() {
        row = row.child(copy_button(view_id, message.id(), this));
    }
    row.into_any_element()
}

fn source_chip(
    view_id: u64,
    message: MessageId,
    index: usize,
    source: &SourceRef,
    this: &WeakEntity<ChatView>,
) -> AnyElement {
    let mut tooltip = source.title.clone();
    if let Some(location) = &source.location {
        tooltip.push_str(&format!("\n{location}"));
    }
    if let Some(score) = source.score {
        tooltip.push_str(&format!("\nscore {score:.2}"));
    }

    let this = this.clone();
    let source = source.clone();
    let id = ElementId::Name(format!("source-{view_id}-{message}-{index}").into());
    Button::new(id)
        .label(source.title.clone())
        .xsmall()
        .ghost()
        .tooltip(tooltip)
        .on_click(move |_, _, cx| {
            let source = source.clone();
            let _ = this.update(cx, |view, cx| view.source_clicked(source, cx));
        })
        .into_any_element()
}

fn copy_button(view_id: u64, message: MessageId, this: &WeakEntity<ChatView>) -> AnyElement {
    let this = this.clone();
    let id = ElementId::Name(format!("copy-{view_id}-{message}").into());
    Button::new(id)
        .icon(IconName::Copy)
        .xsmall()
        .ghost()
        .tooltip("Copy message")
        .on_click(move |_, _, cx| {
            let _ = this.update(cx, |view, cx| view.copy_message(message, cx));
        })
        .into_any_element()
}

fn error_banner(
    message: MessageId,
    error: &str,
    this: &WeakEntity<ChatView>,
    view_id: u64,
    theme: &Theme,
) -> AnyElement {
    let this = this.clone();
    let id = ElementId::Name(format!("retry-{view_id}-{message}").into());
    h_flex()
        .items_center()
        .gap_2()
        .p_2()
        .rounded(px(6.))
        .bg(theme.danger.opacity(0.08))
        .border_1()
        .border_color(theme.danger.opacity(0.25))
        .child(div().text_color(theme.danger).child(IconName::CircleAlert))
        .child(div().flex_1().text_sm().child(error.to_owned()))
        .child(
            Button::new(id)
                .label("Retry")
                .small()
                .outline()
                .on_click(move |_, _, cx| {
                    let _ = this.update(cx, |view, cx| view.retry(message, cx));
                }),
        )
        .into_any_element()
}

fn streaming_dot(view_id: u64, id: MessageId, color: Hsla) -> AnyElement {
    let element_id = ElementId::Name(format!("stream-dot-{view_id}-{id}").into());
    div()
        .size(px(7.))
        .rounded(px(9999.))
        .bg(color)
        .with_animation(
            element_id,
            Animation::new(Duration::from_millis(900)).repeat(),
            |dot, delta| dot.opacity(0.3 + 0.7 * (1.0 - delta)),
        )
        .into_any_element()
}

fn tag(label: &'static str, color: Hsla) -> AnyElement {
    div()
        .px(px(6.))
        .py(px(1.))
        .rounded(px(9999.))
        .bg(color.opacity(0.12))
        .text_xs()
        .text_color(color)
        .child(label)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use gpui_kit::TestAppContext;

    use super::*;

    #[gpui_kit::test]
    fn appending_updates_state_and_text_cache(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let view = cx.new(ChatView::new);

        let (user, assistant) = cx.update(|cx| {
            view.update(cx, |view, cx| {
                let user = view.push_user("Question", cx);
                let assistant = view.begin_assistant(cx);
                assert!(view.append(assistant, "Answer", cx));
                assert!(view.finish(assistant, cx));
                (user, assistant)
            })
        });

        cx.update(|cx| {
            let view = view.read(cx);
            let state = view.state.read(cx);
            assert_eq!(state.len(), 2);
            assert_eq!(state.messages()[0].id(), user);
            assert_eq!(state.messages()[1].id(), assistant);
            assert_eq!(state.messages()[1].text(), "Answer");
            assert_eq!(view.texts.read(cx).len(), 2);
        });
    }

    #[gpui_kit::test]
    fn a_context_note_can_be_inserted_before_the_answer(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let view = cx.new(ChatView::new);

        cx.update(|cx| {
            view.update(cx, |view, cx| {
                view.push_user("Question", cx);
                let assistant = view.begin_assistant(cx);
                assert!(view.push_system_before(assistant, "Attached: @Kano2025", cx));

                let state = view.state.read(cx);
                assert_eq!(state.len(), 3);
                assert_eq!(
                    state
                        .messages()
                        .iter()
                        .map(ChatMessage::role)
                        .collect::<Vec<_>>(),
                    [ChatRole::User, ChatRole::System, ChatRole::Assistant]
                );
                assert_eq!(view.texts.read(cx).len(), 3);
            });
        });
    }

    #[gpui_kit::test]
    fn copying_puts_the_raw_body_on_the_clipboard(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let view = cx.new(ChatView::new);
        let id = cx.update(|cx| view.update(cx, |view, cx| view.push_user("copy me", cx)));

        cx.update(|cx| view.update(cx, |view, cx| view.copy_message(id, cx)));

        let item = cx.read_from_clipboard().expect("clipboard item");
        assert_eq!(item.text().as_deref(), Some("copy me"));
    }

    #[gpui_kit::test]
    fn every_message_state_draws(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let (view, cx) = cx.add_window_view(|_, cx| ChatView::new(cx));

        cx.update(|_, cx| {
            view.update(cx, |view, cx| {
                view.push_user("Question", cx);
                view.push_message(
                    NewMessage::assistant("## Answer\n\nBody.").meta(MessageMeta {
                        model: Some("deepseek-chat".to_owned()),
                        chunks: Some(4),
                        sources: vec![SourceRef::new("paper", "Paper").location("p. 4")],
                        tokens: Some(42),
                        elapsed: Some(Duration::from_millis(1200)),
                    }),
                    cx,
                );
                let streaming = view.begin_assistant(cx);
                let _ = view.append(streaming, "Partial", cx);
                let stopped = view.begin_assistant(cx);
                let _ = view.append(stopped, "Stopped", cx);
                let _ = view.stop(stopped, cx);
                let failed = view.begin_assistant(cx);
                let _ = view.fail(failed, "network error", cx);
                view.push_system("A system notice", cx);
            });
        });

        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });
    }
}
