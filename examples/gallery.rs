//! M1 gallery: every ragrig-gpui primitive in one window.
//!
//! The transcript is seeded with complete, stopped, and failed messages. Send
//! a prompt to stream a mock answer, or switch sessions in the left dock. The
//! right dock shows the chat settings and the agent component list.
//!
//! Run from the crate directory:
//!
//! ```text
//! cargo run --example gallery --no-default-features
//! ```

use std::time::{Duration, Instant};

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, Hsla, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window,
    WindowOptions,
    assets::IconName,
    base::dock::{Panel as BasePanel, PanelEvent},
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        dock::{
            DockArea, DockLayout, DockPlacement, DockSkin, Panel as ComponentPanel, panel_handle,
            register_panel,
        },
        h_flex, v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, size,
};
use ragrig_gpui::prelude::*;

// ---------------------------------------------------------------------------
// Demo state
// ---------------------------------------------------------------------------

struct GalleryState {
    chat: Entity<ChatView>,
    composer: Entity<Composer>,
    settings: Entity<ChatSettings>,
    sessions: Vec<SessionEntry>,
    attachments: Vec<Attachment>,
    current_session: Option<String>,
    status: SharedString,
    streaming: bool,
    generation: u64,
    started_at: Option<Instant>,
}

impl GalleryState {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = cx.new(ChatView::new);
        let composer = cx.new(|cx| Composer::new(window, cx));
        let settings = cx.new(|_| ChatSettings::default());
        let sessions = vec![
            SessionEntry::new("demo", "Demo transcript").detail("just now"),
            SessionEntry::new("methods", "Methods review").detail("2 h ago"),
            SessionEntry::new("empty", "Empty conversation").detail("yesterday"),
        ];
        let attachments = vec![
            Attachment::document("draft", "draft.md"),
            Attachment::bibliography("refs", "references.bib").on(true),
            Attachment::pending("selection", "Selection (312 chars)"),
            Attachment::offer("bib", "refs.bib", "Insert"),
        ];

        let mut this = Self {
            chat,
            composer,
            settings,
            sessions,
            attachments,
            current_session: None,
            status: "Ready".into(),
            streaming: false,
            generation: 0,
            started_at: None,
        };

        cx.subscribe(&this.composer, |this, _, event: &ComposerEvent, cx| {
            this.on_composer(event.clone(), cx);
        })
        .detach();
        cx.subscribe(&this.chat, |this, _, event: &ChatViewEvent, cx| {
            this.on_chat_event(event.clone(), cx);
        })
        .detach();

        this.load_demo(cx);
        this.composer
            .update(cx, |composer, cx| composer.focus(window, cx));
        this
    }

    fn load_demo(&mut self, cx: &mut Context<Self>) {
        self.reset_generation(cx);
        let messages = vec![
            NewMessage::system("Demo transcript. Send a prompt to stream a new answer."),
            NewMessage::user("Summarise the methods section and flag anything that changed."),
            NewMessage::assistant(DEMO_ANSWER).meta(demo_meta()),
            NewMessage::assistant(
                "The larger change is in the retriever: hybrid BM25 and vector search now",
            )
            .state(MessageState::Stopped)
            .meta(MessageMeta {
                model: Some("deepseek-chat".to_owned()),
                chunks: Some(8),
                tokens: Some(96),
                elapsed: Some(Duration::from_millis(2100)),
                ..Default::default()
            }),
            NewMessage::assistant("I could not finish comparing the chunkers.")
                .state(MessageState::Error {
                    message: "The model connection was reset.".to_owned(),
                })
                .meta(MessageMeta {
                    model: Some("deepseek-chat".to_owned()),
                    chunks: Some(3),
                    ..Default::default()
                }),
        ];
        self.chat.update(cx, |chat, cx| chat.replace(messages, cx));
        self.current_session = Some("demo".to_owned());
        self.set_status("Demo transcript loaded", cx);
    }

    fn load_session(&mut self, id: &str, cx: &mut Context<Self>) {
        match id {
            "demo" => self.load_demo(cx),
            "methods" => {
                self.reset_generation(cx);
                let messages = vec![
                    NewMessage::system("Session loaded from the demo store."),
                    NewMessage::user("What changed in the chunker?"),
                    NewMessage::assistant(
                        "The chunker now keeps table headers with each chunk and overlaps by \
                         one paragraph. That improves answers about tables without increasing \
                         the retrieval budget.",
                    )
                    .meta(MessageMeta {
                        model: Some("qwen3:8b".to_owned()),
                        chunks: Some(5),
                        sources: vec![SourceRef::new("methods", "methods.pdf").location("p. 7")],
                        tokens: Some(148),
                        elapsed: Some(Duration::from_millis(2600)),
                    }),
                ];
                self.chat.update(cx, |chat, cx| chat.replace(messages, cx));
                self.current_session = Some("methods".to_owned());
                self.set_status("Loaded Methods review", cx);
            }
            "empty" => {
                self.new_session(cx);
                self.current_session = Some("empty".to_owned());
            }
            _ => {}
        }
    }

    fn new_session(&mut self, cx: &mut Context<Self>) {
        self.reset_generation(cx);
        self.chat.update(cx, |chat, cx| chat.clear(cx));
        self.current_session = None;
        self.set_status("New conversation", cx);
    }

    fn start_answer(&mut self, prompt: String, cx: &mut Context<Self>) {
        if self.streaming {
            return;
        }

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.streaming = true;
        self.started_at = Some(Instant::now());
        self.status = "Generating…".into();

        self.chat.update(cx, |chat, cx| {
            chat.push_user(prompt.clone(), cx);
        });
        let answer = self.chat.update(cx, |chat, cx| chat.begin_assistant(cx));
        self.composer
            .update(cx, |composer, cx| composer.set_streaming(true, cx));

        let tokens = mock_answer(&prompt)
            .split_inclusive(char::is_whitespace)
            .map(str::to_owned)
            .collect::<Vec<_>>();

        cx.spawn(async move |this, cx| {
            for token in tokens {
                cx.background_executor()
                    .timer(Duration::from_millis(26))
                    .await;
                let alive = this
                    .update(cx, |state, cx| {
                        if state.generation != generation || !state.streaming {
                            return false;
                        }
                        state.chat.update(cx, |chat, cx| {
                            let _ = chat.append(answer, &token, cx);
                        });
                        true
                    })
                    .unwrap_or(false);
                if !alive {
                    return;
                }
            }
            let _ = this.update(cx, |state, cx| {
                if state.generation == generation {
                    state.finish_answer(answer, cx);
                }
            });
        })
        .detach();

        cx.notify();
    }

    fn finish_answer(&mut self, id: MessageId, cx: &mut Context<Self>) {
        if !self.streaming {
            return;
        }
        self.streaming = false;
        self.composer
            .update(cx, |composer, cx| composer.set_streaming(false, cx));

        let elapsed = self.started_at.take().map(|started| started.elapsed());
        let model = self.settings.read(cx).model.clone();
        let tokens = self
            .chat
            .read(cx)
            .state()
            .read(cx)
            .get(id)
            .map(|message| message.text().split_whitespace().count());

        self.chat.update(cx, |chat, cx| {
            chat.set_meta(
                id,
                MessageMeta {
                    model: Some(model),
                    chunks: Some(6),
                    sources: vec![
                        SourceRef::new("methods", "methods.pdf").location("p. 4"),
                        SourceRef::new("appendix", "appendix.pdf").location("p. 12"),
                    ],
                    tokens,
                    elapsed,
                },
                cx,
            );
            chat.finish(id, cx);
        });
        self.set_status("Answer complete", cx);
    }

    fn stop_answer(&mut self, cx: &mut Context<Self>) {
        if !self.streaming {
            return;
        }
        self.generation = self.generation.wrapping_add(1);
        self.streaming = false;
        self.started_at = None;
        self.composer
            .update(cx, |composer, cx| composer.set_streaming(false, cx));

        let id = self.chat.read(cx).state().read(cx).streaming_id();
        if let Some(id) = id {
            self.chat.update(cx, |chat, cx| {
                chat.stop(id, cx);
            });
        }
        self.set_status("Stopped", cx);
    }

    fn on_composer(&mut self, event: ComposerEvent, cx: &mut Context<Self>) {
        match event {
            ComposerEvent::Submit(text) => self.start_answer(text, cx),
            ComposerEvent::Stop => self.stop_answer(cx),
        }
    }

    fn on_chat_event(&mut self, event: ChatViewEvent, cx: &mut Context<Self>) {
        match event {
            ChatViewEvent::Copied(_) => self.set_status("Message copied", cx),
            ChatViewEvent::Retry(id) => {
                self.set_status(format!("Retry requested for message {id}"), cx)
            }
            ChatViewEvent::SourceClicked(source) => {
                self.set_status(format!("Open source: {}", source.title), cx)
            }
        }
    }

    fn on_attachment(&mut self, event: AttachmentEvent, cx: &mut Context<Self>) {
        match event {
            AttachmentEvent::Toggled(id) => {
                if let Some(attachment) = self.attachments.iter_mut().find(|a| a.id == id) {
                    let next = !attachment.is_on();
                    *attachment = attachment.clone().on(next);
                }
                self.set_status(format!("Toggled attachment {id}"), cx);
            }
            AttachmentEvent::Dropped(id) => {
                self.attachments.retain(|attachment| attachment.id != id);
                self.set_status(format!("Dropped attachment {id}"), cx);
            }
            AttachmentEvent::Accepted(id) => {
                self.set_status(format!("Accepted attachment {id}"), cx);
            }
        }
    }

    fn reset_generation(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.streaming = false;
        self.started_at = None;
        self.composer
            .update(cx, |composer, cx| composer.set_streaming(false, cx));
    }

    fn set_status(&mut self, status: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = status.into();
        cx.notify();
    }

    fn agent_components(&self, cx: &App) -> Vec<AgentComponent> {
        let model = self.settings.read(cx).model.clone();
        vec![
            AgentComponent::new("chat", ComponentKind::Chat, model)
                .status(if self.streaming {
                    ComponentStatus::Busy
                } else {
                    ComponentStatus::Ready
                })
                .detail("OpenAI-compatible endpoint"),
            AgentComponent::new("embed", ComponentKind::Embedder, "nomic-embed-text")
                .status(ComponentStatus::Ready)
                .detail("768 dimensions"),
            AgentComponent::new("store", ComponentKind::Store, "Local index")
                .status(ComponentStatus::Ready)
                .detail("42 documents"),
            AgentComponent::new("parser", ComponentKind::Parser, "GROBID")
                .status(ComponentStatus::Inactive)
                .detail("Feature-gated in M4"),
        ]
    }
}

fn demo_meta() -> MessageMeta {
    MessageMeta {
        model: Some("deepseek-chat".to_owned()),
        chunks: Some(8),
        sources: vec![
            SourceRef::new("methods", "methods.pdf").location("p. 4"),
            SourceRef::new("appendix", "appendix.pdf").location("p. 12"),
        ],
        tokens: Some(312),
        elapsed: Some(Duration::from_millis(4200)),
    }
}

const DEMO_ANSWER: &str = r#"## Methods review

The pipeline has **three** stages:

| Stage | Output |
|---|---|
| Parse | Structured text |
| Chunk | 1,200-token windows |
| Retrieve | Top-8 passages |

The retriever changed from pure vector search to a hybrid of BM25 and
embeddings. The chunker keeps table headers, and the parser now reports page
numbers with each block.

```rust
let context = retrieve(&question, 8);
```

Nothing else in the pipeline changed."#;

fn mock_answer(prompt: &str) -> String {
    format!(
        r#"## Mock answer

You asked: **{prompt}**

The methods section describes a three-stage pipeline:

1. Parse the PDF into structured text.
2. Embed overlapping chunks and store them in the local index.
3. Retrieve the strongest passages and cite them in the prompt.

```rust
let context = retrieve(&question, 8);
```

Retrieval currently uses hybrid BM25 and vector search. The demo marks the
parser as inactive because GROBID arrives in M4.

_This answer is generated locally by the M1 gallery, not by Ragrig._
"#
    )
}

// ---------------------------------------------------------------------------
// Chat panel
// ---------------------------------------------------------------------------

struct ChatPanel {
    state: Entity<GalleryState>,
    focus_handle: FocusHandle,
}

impl ChatPanel {
    fn new(state: Entity<GalleryState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (components, status, sessions, current, chat, composer, attachments) = {
            let state = self.state.read(cx);
            (
                state.agent_components(cx),
                state.status.clone(),
                state.sessions.clone(),
                state.current_session.clone(),
                state.chat.clone(),
                state.composer.clone(),
                state.attachments.clone(),
            )
        };

        let agent_state = self.state.clone();
        let agent_bar = AgentBar::new("gallery-agent", components).on_event(move |event, _, cx| {
            agent_state.update(cx, |state, cx| match event {
                AgentBarEvent::Configure(id) => {
                    state.set_status(format!("Configure {id}"), cx);
                }
                AgentBarEvent::OpenSettings => {
                    state.set_status("Settings are in the right dock", cx);
                }
            });
        });

        let picker_state = self.state.clone();
        let mut picker = SessionPicker::new("gallery-sessions", sessions);
        if let Some(current) = current {
            picker = picker.current(current);
        }
        let picker = picker.on_event(move |event, _, cx| {
            picker_state.update(cx, |state, cx| match event {
                SessionPickerEvent::New => state.new_session(cx),
                SessionPickerEvent::Load(id) => state.load_session(&id, cx),
                SessionPickerEvent::Refresh => state.set_status("Sessions refreshed", cx),
            });
        });

        let attachment_state = self.state.clone();
        let attachments = AttachmentRows::new(attachments).on_event(move |event, _, cx| {
            attachment_state.update(cx, |state, cx| state.on_attachment(event, cx));
        });

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(
                div()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(agent_bar),
            )
            .child(
                h_flex()
                    .px_3()
                    .py_2()
                    .gap_3()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(picker)
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(status),
                    ),
            )
            .child(div().flex_1().min_h(px(0.)).child(chat))
            .child(div().px_3().py_2().child(attachments))
            .child(composer)
    }
}

impl EventEmitter<PanelEvent> for ChatPanel {}

impl Focusable for ChatPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ChatPanel {
    fn panel_name(&self) -> &'static str {
        "gallery-chat"
    }
}

impl ComponentPanel for ChatPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Chat".into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_2().child(IconName::Bot).child("Chat")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Library and sessions panels
// ---------------------------------------------------------------------------

struct LibraryPanel {
    state: Entity<GalleryState>,
    focus_handle: FocusHandle,
}

impl LibraryPanel {
    fn new(state: Entity<GalleryState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let demo_state = self.state.clone();
        let clear_state = self.state.clone();
        let demo = Button::new("gallery-load-demo")
            .label("Demo")
            .icon(IconName::Sparkles)
            .small()
            .outline()
            .on_click(move |_, _, cx| {
                demo_state.update(cx, |state, cx| state.load_demo(cx));
            });
        let clear = Button::new("gallery-clear")
            .label("Clear")
            .icon(IconName::Trash)
            .small()
            .ghost()
            .on_click(move |_, _, cx| {
                clear_state.update(cx, |state, cx| state.new_session(cx));
            });

        SidePanel::new("gallery-library", "Library")
            .subtitle("Corpora and documents")
            .action(demo)
            .action(clear)
            .child(document_row(
                IconName::FileText,
                "methods.pdf",
                "42 chunks · indexed",
                cx,
            ))
            .child(document_row(
                IconName::FileText,
                "appendix.pdf",
                "17 chunks · indexed",
                cx,
            ))
            .child(document_row(
                IconName::BookOpen,
                "references.bib",
                "28 entries · attached",
                cx,
            ))
            .footer(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Corpus management arrives in M3."),
            )
    }
}

impl EventEmitter<PanelEvent> for LibraryPanel {}

impl Focusable for LibraryPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for LibraryPanel {
    fn panel_name(&self) -> &'static str {
        "gallery-library"
    }
}

impl ComponentPanel for LibraryPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Library".into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_2().child(IconName::Library).child("Library")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

fn document_row(icon: IconName, title: &str, detail: &str, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .gap_2()
        .p_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .child(div().text_color(theme.muted_foreground).child(icon))
        .child(
            v_flex()
                .gap_1()
                .child(div().text_sm().child(title.to_owned()))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(detail.to_owned()),
                ),
        )
        .into_any_element()
}

struct SessionsPanel {
    state: Entity<GalleryState>,
    focus_handle: FocusHandle,
}

impl SessionsPanel {
    fn new(state: Entity<GalleryState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for SessionsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (sessions, current) = {
            let state = self.state.read(cx);
            (state.sessions.clone(), state.current_session.clone())
        };

        let new_state = self.state.clone();
        let new_session = Button::new("gallery-new-session")
            .label("New")
            .icon(IconName::Plus)
            .small()
            .outline()
            .on_click(move |_, _, cx| {
                new_state.update(cx, |state, cx| state.new_session(cx));
            });

        let mut list = v_flex().gap_2();
        for (index, session) in sessions.into_iter().enumerate() {
            let is_current = current.as_deref() == Some(session.id.as_str());
            let click_state = self.state.clone();
            let id = session.id.clone();
            let button = Button::new(ElementId::Name(format!("gallery-session-{index}").into()))
                .icon(if is_current {
                    IconName::Check
                } else {
                    IconName::MessagesSquare
                })
                .label(session.label.clone())
                .ghost()
                .small()
                .w_full()
                .on_click(move |_, _, cx| {
                    click_state.update(cx, |state, cx| state.load_session(&id, cx));
                });
            let button = if is_current { button.primary() } else { button };
            list = list.child(button);
            if let Some(detail) = session.detail {
                list = list.child(
                    div()
                        .px_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                );
            }
        }

        SidePanel::new("gallery-session-list", "Sessions")
            .subtitle("Saved conversations")
            .action(new_session)
            .child(list)
            .footer(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Session storage is owned by the host."),
            )
    }
}

impl EventEmitter<PanelEvent> for SessionsPanel {}

impl Focusable for SessionsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for SessionsPanel {
    fn panel_name(&self) -> &'static str {
        "gallery-sessions"
    }
}

impl ComponentPanel for SessionsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Sessions".into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .gap_2()
            .child(IconName::MessagesSquare)
            .child("Sessions")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Settings and component panels
// ---------------------------------------------------------------------------

struct SettingsPanel {
    state: Entity<GalleryState>,
    focus_handle: FocusHandle,
}

impl SettingsPanel {
    fn new(state: Entity<GalleryState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let settings = self.state.read(cx).settings.clone();
        div()
            .size_full()
            .bg(cx.theme().background)
            .child(chat_settings("gallery-settings", settings))
    }
}

impl EventEmitter<PanelEvent> for SettingsPanel {}

impl Focusable for SettingsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for SettingsPanel {
    fn panel_name(&self) -> &'static str {
        "gallery-settings"
    }
}

impl ComponentPanel for SettingsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Settings".into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_2().child(IconName::Settings).child("Settings")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

struct ComponentsPanel {
    state: Entity<GalleryState>,
    focus_handle: FocusHandle,
}

impl ComponentsPanel {
    fn new(state: Entity<GalleryState>, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for ComponentsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let components = self.state.read(cx).agent_components(cx);
        SidePanel::new("gallery-components", "Components")
            .subtitle("Live status of the mock pipeline")
            .children(
                components
                    .iter()
                    .map(|component| component_card(component, cx)),
            )
    }
}

fn component_card(component: &AgentComponent, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = component_color(component.status, cx);
    h_flex()
        .gap_2()
        .p_2()
        .rounded(theme.radius)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .text_color(theme.muted_foreground)
                .child(component.kind.icon()),
        )
        .child(
            v_flex()
                .gap_1()
                .flex_1()
                .child(div().text_sm().child(component.label.clone()))
                .when_some(component.detail.clone(), |card, detail| {
                    card.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(detail),
                    )
                }),
        )
        .child(
            h_flex()
                .gap_1()
                .child(div().size(px(7.)).rounded(px(9999.)).bg(color))
                .child(
                    div()
                        .text_xs()
                        .text_color(color)
                        .child(component.status.label()),
                ),
        )
        .into_any_element()
}

fn component_color(status: ComponentStatus, cx: &App) -> Hsla {
    let theme = cx.theme();
    match status {
        ComponentStatus::Inactive => theme.muted_foreground,
        ComponentStatus::Ready => theme.success,
        ComponentStatus::Busy => theme.info,
        ComponentStatus::Error => theme.danger,
    }
}

impl EventEmitter<PanelEvent> for ComponentsPanel {}

impl Focusable for ComponentsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ComponentsPanel {
    fn panel_name(&self) -> &'static str {
        "gallery-components"
    }
}

impl ComponentPanel for ComponentsPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Components".into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_2().child(IconName::Cpu).child("Components")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

struct GalleryApp {
    /// Kept for the gallery's tests; the panels own their clones.
    #[allow(dead_code)]
    state: Entity<GalleryState>,
    dock: Entity<DockArea>,
}

impl GalleryApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| GalleryState::new(window, cx));

        {
            let state = state.clone();
            register_panel(cx, "gallery-chat", move |_, _, cx| {
                panel_handle(cx.new(|cx| ChatPanel::new(state.clone(), cx)))
            });
        }
        {
            let state = state.clone();
            register_panel(cx, "gallery-library", move |_, _, cx| {
                panel_handle(cx.new(|cx| LibraryPanel::new(state.clone(), cx)))
            });
        }
        {
            let state = state.clone();
            register_panel(cx, "gallery-sessions", move |_, _, cx| {
                panel_handle(cx.new(|cx| SessionsPanel::new(state.clone(), cx)))
            });
        }
        {
            let state = state.clone();
            register_panel(cx, "gallery-settings", move |_, _, cx| {
                panel_handle(cx.new(|cx| SettingsPanel::new(state.clone(), cx)))
            });
        }
        {
            let state = state.clone();
            register_panel(cx, "gallery-components", move |_, _, cx| {
                panel_handle(cx.new(|cx| ComponentsPanel::new(state.clone(), cx)))
            });
        }

        let chat = cx.new(|cx| ChatPanel::new(state.clone(), cx));
        let library = cx.new(|cx| LibraryPanel::new(state.clone(), cx));
        let sessions = cx.new(|cx| SessionsPanel::new(state.clone(), cx));
        let settings = cx.new(|cx| SettingsPanel::new(state.clone(), cx));
        let components = cx.new(|cx| ComponentsPanel::new(state.clone(), cx));

        let (dock, _skin) = DockSkin::dock_area("gallery-dock", Some(1), window, cx);
        dock.update(cx, |dock, cx| {
            dock.set_center(
                DockLayout::tabs().panel_view(panel_handle(chat), cx),
                window,
                cx,
            );
            dock.set_dock(
                DockPlacement::Left,
                DockLayout::tabs()
                    .panel_view(panel_handle(library), cx)
                    .panel_view(panel_handle(sessions), cx),
                window,
                cx,
            );
            dock.set_dock(
                DockPlacement::Right,
                DockLayout::tabs()
                    .panel_view(panel_handle(settings), cx)
                    .panel_view(panel_handle(components), cx),
                window,
                cx,
            );
            dock.set_dock_size(DockPlacement::Left, px(280.), window, cx);
            dock.set_dock_size(DockPlacement::Right, px(680.), window, cx);
        });

        Self { state, dock }
    }
}

impl Render for GalleryApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .bg(cx.theme().background)
            .text_color(cx.theme().foreground)
            .child(self.dock.clone())
    }
}

fn main() {
    gpui_kit::application()
        .with_assets(gpui_kit::assets::AllAssets)
        .run(|cx| {
            ragrig_gpui::init(cx);

            let restore = gpui_kit::Bounds::centered(None, size(px(1600.), px(1000.)), cx);
            let options = WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(restore)),
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some("ragrig-gpui — M1 gallery".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };

            if let Err(error) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| GalleryApp::new(window, cx))
            }) {
                eprintln!("failed to open the gallery window: {error:#}");
            }
        });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::TestAppContext;

    #[gpui_kit::test]
    fn the_gallery_mounts_with_the_demo_transcript(cx: &mut TestAppContext) {
        cx.update(ragrig_gpui::init);
        let (app, cx) = cx.add_window_view(GalleryApp::new);

        cx.run_until_parked();
        cx.update(|window, cx| {
            window.draw(cx).clear(cx);
        });

        cx.update(|_, cx| {
            let state = app.read(cx).state.clone();
            let chat = state.read(cx).chat.clone();
            assert!(chat.read(cx).len(cx) >= 4);
        });
    }

    #[gpui_kit::test]
    fn a_mock_answer_streams_to_completion(cx: &mut TestAppContext) {
        cx.update(ragrig_gpui::init);
        let (app, cx) = cx.add_window_view(GalleryApp::new);
        let state = cx.update(|_, cx| app.read(cx).state.clone());

        cx.update(|_, cx| {
            state.update(cx, |state, cx| {
                state.start_answer("test the stream".into(), cx)
            });
        });

        for _ in 0..120 {
            cx.background_executor
                .advance_clock(Duration::from_millis(30));
            cx.run_until_parked();
        }

        cx.update(|_, cx| {
            let state = state.read(cx);
            let chat = state.chat.read(cx).state().read(cx);
            let answer = chat.last_assistant().expect("assistant answer");
            assert_eq!(answer.state(), &MessageState::Complete);
            assert!(answer.text().contains("Mock answer"));
            assert!(!state.streaming);
        });
    }
}
