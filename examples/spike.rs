//! M0 foundation spike for `ragrig-gpui`.
//!
//! Proves the pieces Scico's RAG surface depends on:
//!
//! - a `DockArea` with center, left, and right panels;
//! - live Markdown in a tail-following `MessageScroller`;
//! - an auto-growing `Textarea` whose Ctrl/⌘+Enter submits;
//! - a dedicated Tokio thread streaming batches over an async channel;
//! - cancellation through [`ragrig_gpui::CancelFlag`];
//! - a `Settings` page and a `Popover`.
//!
//! Run with:
//!
//! ```text
//! cargo run --example spike --no-default-features
//! ```

use std::time::Duration;

use async_channel::{Receiver, Sender, TrySendError, unbounded};
use gpui_kit::{
    AnyElement, App, AppContext as _, Axis, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, IntoElement, ParentElement as _, Render, SharedString, Styled as _,
    Window, WindowOptions,
    assets::IconName,
    base::dock::Panel as BasePanel,
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        dock::{
            DockArea, DockLayout, DockPlacement, DockSkin, Panel as ComponentPanel, PanelEvent,
            panel_handle,
        },
        h_flex,
        input::{InputEvent, Textarea, TextareaState},
        message_scroller::{MessageScroller, MessageScrollerState},
        popover::Popover,
        setting::{
            NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
        },
        text::{TextView, TextViewStyle},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, size,
};
use ragrig_gpui::CancelFlag;

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug)]
struct ChatMessage {
    role: Role,
    content: String,
    stopped: bool,
}

#[derive(Default)]
struct ChatModel {
    messages: Vec<ChatMessage>,
    streaming: bool,
}

impl ChatModel {
    /// Appends the user prompt and an empty assistant row; returns its index.
    fn begin(&mut self, prompt: &str) -> usize {
        self.messages.push(ChatMessage {
            role: Role::User,
            content: prompt.to_owned(),
            stopped: false,
        });
        self.messages.push(ChatMessage {
            role: Role::Assistant,
            content: String::new(),
            stopped: false,
        });
        self.streaming = true;
        self.messages.len() - 1
    }

    /// Appends one streamed batch to an assistant row.
    fn push_batch(&mut self, index: usize, batch: &str) {
        if !self.streaming {
            return;
        }
        if let Some(message) = self.messages.get_mut(index) {
            message.content.push_str(batch);
        }
    }

    /// Ends the stream, keeping whatever content already arrived.
    fn finish(&mut self, index: usize, stopped: bool) {
        self.streaming = false;
        if let Some(message) = self.messages.get_mut(index) {
            message.stopped = stopped;
        }
    }
}

// ---------------------------------------------------------------------------
// Worker bridge
// ---------------------------------------------------------------------------

#[derive(Debug)]
enum BridgeKind {
    Token(String),
    Done,
    Cancelled,
}

#[derive(Debug)]
struct BridgeEvent {
    generation: u64,
    kind: BridgeKind,
    sent_at: std::time::Instant,
}

struct StreamJob {
    prompt: String,
    generation: u64,
    events: Sender<BridgeEvent>,
    cancel: CancelFlag,
}

/// Starts a dedicated Tokio thread. The thread exits when every job sender is
/// dropped, which happens when the owning panel is dropped.
fn spawn_worker(jobs: Receiver<StreamJob>) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("ragrig-spike".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    eprintln!("ragrig spike: cannot create the Tokio runtime: {error}");
                    return;
                }
            };

            runtime.block_on(async move {
                while let Ok(job) = jobs.recv().await {
                    tokio::spawn(run_job(job));
                }
            });
        })
        .map(|_| ())
}

async fn run_job(job: StreamJob) {
    let answer = fake_answer(&job.prompt);
    let mut batch = String::new();
    let mut since_flush = 0_u8;

    tokio::time::sleep(Duration::from_millis(180)).await;

    for token in answer.split_inclusive(char::is_whitespace) {
        if job.cancel.is_cancelled() {
            send(&job, BridgeKind::Cancelled).await;
            return;
        }

        batch.push_str(token);
        since_flush += 1;

        // Four word-sized chunks every 30 ms is roughly 30–35 UI updates per
        // second, the order of magnitude M2's token batcher will use.
        if since_flush == 4 {
            since_flush = 0;
            if !send(&job, BridgeKind::Token(std::mem::take(&mut batch))).await {
                return;
            }
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    if !batch.is_empty() && !send(&job, BridgeKind::Token(batch)).await {
        return;
    }
    send(&job, BridgeKind::Done).await;
}

/// Returns `false` when the UI side has gone away.
async fn send(job: &StreamJob, kind: BridgeKind) -> bool {
    job.events
        .send(BridgeEvent {
            generation: job.generation,
            kind,
            sent_at: std::time::Instant::now(),
        })
        .await
        .is_ok()
}

fn fake_answer(prompt: &str) -> String {
    format!(
        r#"## Port spike

You asked: **{prompt}**

This answer is produced on a dedicated Tokio thread and crosses into GPUI in
small batches through an `async-channel`.

- Markdown renders while the answer streams
- The transcript follows the tail until you scroll up
- **Stop** keeps the partial answer

```rust
let (tx, rx) = async_channel::unbounded();
```

Press **Ctrl/⌘+Enter** to send and **Enter** for a new line.
"#
    )
}

// ---------------------------------------------------------------------------
// Chat panel
// ---------------------------------------------------------------------------

struct ChatPanel {
    focus_handle: FocusHandle,
    model: Entity<ChatModel>,
    transcript: Entity<MessageScrollerState>,
    composer: Entity<TextareaState>,
    jobs: Sender<StreamJob>,
    events_tx: Sender<BridgeEvent>,
    cancel: CancelFlag,
    generation: u64,
    pending_clear: bool,
    autostart: Option<String>,
    metrics: bool,
    created_at: std::time::Instant,
}

impl ChatPanel {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let model = cx.new(|_| ChatModel::default());
        let transcript = cx.new(|cx| MessageScrollerState::new(0, cx));
        let composer = cx.new(|cx| TextareaState::new(window, cx).auto_grow(1, 6));

        let (jobs, jobs_rx) = unbounded();
        if let Err(error) = spawn_worker(jobs_rx) {
            eprintln!("ragrig spike: cannot start the streaming worker: {error}");
        }

        let (events_tx, events_rx) = unbounded();
        cx.subscribe(&composer, |this, _, event: &InputEvent, cx| {
            if let InputEvent::PressEnter {
                secondary: true, ..
            } = event
            {
                this.submit(cx);
            }
        })
        .detach();

        cx.spawn(async move |this, cx| {
            while let Ok(event) = events_rx.recv().await {
                if this
                    .update(cx, |panel, cx| panel.on_bridge_event(event, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        composer.update(cx, |state, cx| state.focus(window, cx));

        Self {
            focus_handle: cx.focus_handle(),
            model,
            transcript,
            composer,
            jobs,
            events_tx,
            cancel: CancelFlag::new(),
            generation: 0,
            pending_clear: false,
            autostart: std::env::var("RAGRIG_SPIKE_AUTORUN")
                .ok()
                .filter(|prompt| !prompt.trim().is_empty()),
            metrics: std::env::var_os("RAGRIG_SPIKE_METRICS").is_some(),
            created_at: std::time::Instant::now(),
        }
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        let prompt = self.composer.read(cx).value().trim().to_owned();
        if prompt.is_empty() || self.model.read(cx).streaming {
            return;
        }

        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.cancel.reset();

        let last = self.model.update(cx, |model, _| model.begin(&prompt));
        self.transcript.update(cx, |state, cx| state.append(2, cx));
        self.pending_clear = true;

        let job = StreamJob {
            prompt,
            generation,
            events: self.events_tx.clone(),
            cancel: self.cancel.clone(),
        };

        if let Err(error) = self.jobs.try_send(job) {
            let reason = match error {
                TrySendError::Full(_) => "the worker queue is full".to_owned(),
                TrySendError::Closed(_) => "the worker is not running".to_owned(),
            };
            self.model.update(cx, |model, _| {
                model.push_batch(last, &format!("\n\n_Streaming unavailable: {reason}._"));
                model.finish(last, true);
            });
            self.remeasure(last, cx);
        }

        cx.notify();
    }

    fn stop(&mut self, cx: &mut Context<Self>) {
        if !self.model.read(cx).streaming {
            return;
        }

        self.cancel.cancel();
        // Invalidate events already in flight from the cancelled generation.
        self.generation = self.generation.wrapping_add(1);
        let last = self.model.read(cx).messages.len().saturating_sub(1);
        self.model.update(cx, |model, _| model.finish(last, true));
        self.remeasure(last, cx);
        cx.notify();
    }

    fn on_bridge_event(&mut self, event: BridgeEvent, cx: &mut Context<Self>) {
        if event.generation != self.generation {
            return;
        }

        let kind = match &event.kind {
            BridgeKind::Token(_) => "token",
            BridgeKind::Done => "done",
            BridgeKind::Cancelled => "cancelled",
        };
        if self.metrics {
            eprintln!(
                "ragrig bridge: {kind} at={:?} lag={:?}",
                self.created_at.elapsed(),
                event.sent_at.elapsed()
            );
        }

        let last = self.model.read(cx).messages.len().saturating_sub(1);
        if self.model.read(cx).messages.is_empty() {
            return;
        }

        match event.kind {
            BridgeKind::Token(text) => {
                self.model
                    .update(cx, |model, _| model.push_batch(last, &text));
            }
            BridgeKind::Done => self.model.update(cx, |model, _| model.finish(last, false)),
            BridgeKind::Cancelled => self.model.update(cx, |model, _| model.finish(last, true)),
        }

        self.remeasure(last, cx);
        cx.notify();
    }

    fn remeasure(&self, index: usize, cx: &mut Context<Self>) {
        self.transcript
            .update(cx, |state, cx| state.remeasure_items(index..index + 1, cx));
    }
}

impl Render for ChatPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(prompt) = self.autostart.take() {
            self.composer
                .update(cx, |state, cx| state.set_value(prompt, window, cx));
            self.submit(cx);
        }

        if std::mem::take(&mut self.pending_clear) {
            self.composer
                .update(cx, |state, cx| state.clean(window, cx));
        }

        let (streaming, message_count, composer_empty, last_len) = {
            let model = self.model.read(cx);
            let composer = self.composer.read(cx);
            (
                model.streaming,
                model.messages.len(),
                composer.value().trim().is_empty(),
                model
                    .messages
                    .last()
                    .map_or(0, |message| message.content.len()),
            )
        };

        let agent_bar = h_flex()
            .px_3()
            .py_2()
            .gap_3()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                Popover::new("agent-popover")
                    .trigger(
                        Button::new("agent")
                            .label("Fake RAG")
                            .icon(IconName::Bot)
                            .small(),
                    )
                    .content(|_, _, cx| {
                        v_flex()
                            .p_2()
                            .gap_1()
                            .w(px(240.))
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Agent"),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(cx.theme().muted_foreground)
                                    .child("Mock streaming agent (M0)"),
                            )
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "{message_count} message{} · {last_len} chars · {}",
                        if message_count == 1 { "" } else { "s" },
                        if streaming { "streaming" } else { "idle" }
                    )),
            );

        let transcript: AnyElement =
            if message_count == 0 {
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .child(div().text_2xl().child(IconName::Sparkles))
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Ask your library"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("This spike streams a fake answer from a Tokio worker."),
                    )
                    .into_any_element()
            } else {
                let model = self.model.clone();
                MessageScroller::new("transcript", self.transcript.clone(), move |ix, _, cx| {
                    match model.read(cx).messages.get(ix).cloned() {
                        Some(message) => message_row(ix, &message, cx),
                        None => div().into_any_element(),
                    }
                })
                .scrollbar(true)
                .jump_button(true)
                .with_jump_button_label("Latest")
                .with_bottom_fade(cx.theme().background)
                .flex_1()
                .into_any_element()
            };

        let action = if streaming {
            Button::new("stop")
                .icon(IconName::Square)
                .label("Stop")
                .small()
                .on_click(cx.listener(|this, _, _, cx| this.stop(cx)))
        } else {
            Button::new("send")
                .icon(IconName::Send)
                .label("Send")
                .primary()
                .small()
                .disabled(composer_empty)
                .on_click(cx.listener(|this, _, _, cx| this.submit(cx)))
        };

        let composer_area = v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(Textarea::new(&self.composer).bordered(true))
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("Ctrl/⌘+Enter to send · Enter for a new line"),
                    )
                    .child(action),
            );

        v_flex()
            .size_full()
            .bg(cx.theme().background)
            .child(agent_bar)
            .child(transcript)
            .child(composer_area)
    }
}

fn message_row(index: usize, message: &ChatMessage, cx: &mut App) -> AnyElement {
    let theme = cx.theme();
    let is_user = message.role == Role::User;
    let (icon, label) = if is_user {
        (IconName::CircleUser, "You")
    } else {
        (IconName::Bot, "Ragrig")
    };

    let style = TextViewStyle {
        is_dark: theme.is_dark(),
        ..Default::default()
    };

    let row = v_flex()
        .w_full()
        .gap_2()
        .when(is_user, |row| {
            row.mx_4().p_3().bg(theme.secondary).rounded(theme.radius)
        })
        .when(!is_user, |row| row.px_4().py_3())
        .child(
            h_flex()
                .gap_2()
                .child(
                    div()
                        .text_color(if is_user {
                            theme.muted_foreground
                        } else {
                            theme.primary
                        })
                        .child(icon),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(label),
                )
                .when(message.stopped, |row| {
                    row.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("stopped"),
                    )
                }),
        )
        .child(
            TextView::markdown(
                ElementId::named_usize("spike-message", index),
                message.content.clone(),
            )
            .style(style)
            .selectable(true),
        );

    if !is_user && message.content.is_empty() {
        row.child(
            div()
                .text_sm()
                .text_color(theme.muted_foreground)
                .child("Thinking…"),
        )
        .into_any_element()
    } else {
        row.into_any_element()
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
        "spike-chat"
    }
}

impl ComponentPanel for ChatPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Chat".into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_2().child(IconName::Bot).child("Rag chat")
    }

    fn inner_padding(&self, _: &App) -> bool {
        false
    }
}

// ---------------------------------------------------------------------------
// Placeholder library panel
// ---------------------------------------------------------------------------

struct LibraryPanel {
    focus_handle: FocusHandle,
}

impl LibraryPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_3()
            .gap_3()
            .bg(cx.theme().background)
            .child(
                div()
                    .text_lg()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Library"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Corpus management arrives in M3."),
            )
            .child(
                Button::new("add-files")
                    .label("Add files")
                    .icon(IconName::Plus)
                    .small()
                    .disabled(true),
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
        "spike-library"
    }
}

impl ComponentPanel for LibraryPanel {
    fn tab_name(&self, _: &App) -> Option<SharedString> {
        Some("Library".into())
    }

    fn title(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        h_flex().gap_2().child(IconName::Library).child("Library")
    }
}

// ---------------------------------------------------------------------------
// Settings panel
// ---------------------------------------------------------------------------

struct SettingsPanel {
    focus_handle: FocusHandle,
    model: SharedString,
    system_prompt: SharedString,
    temperature: f64,
    stream: bool,
}

impl SettingsPanel {
    fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            model: "deepseek-chat".into(),
            system_prompt: "Answer from the indexed corpus and cite sources.".into(),
            temperature: 0.2,
            stream: true,
        }
    }

    fn page(&self, cx: &mut Context<Self>) -> SettingPage {
        let entity = cx.entity();

        let model = SettingField::dropdown(
            vec![
                ("deepseek-chat".into(), "DeepSeek Chat".into()),
                ("qwen3:8b".into(), "Qwen3 8B (Ollama)".into()),
                ("gpt-oss:20b".into(), "GPT-OSS 20B (Ollama)".into()),
            ],
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).model.clone()
            },
            {
                let entity = entity.clone();
                move |value: SharedString, cx: &mut App| {
                    entity.update(cx, |this, cx| {
                        this.model = value;
                        cx.notify();
                    });
                }
            },
        );

        let system_prompt = SettingField::input(
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).system_prompt.clone()
            },
            {
                let entity = entity.clone();
                move |value: SharedString, cx: &mut App| {
                    entity.update(cx, |this, cx| {
                        this.system_prompt = value;
                        cx.notify();
                    });
                }
            },
        );

        let temperature = SettingField::number_input(
            NumberFieldOptions {
                min: 0.0,
                max: 2.0,
                step: 0.1,
            },
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).temperature
            },
            {
                let entity = entity.clone();
                move |value: f64, cx: &mut App| {
                    entity.update(cx, |this, cx| {
                        this.temperature = value;
                        cx.notify();
                    });
                }
            },
        );

        let stream = SettingField::switch(
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).stream
            },
            {
                let entity = entity.clone();
                move |value: bool, cx: &mut App| {
                    entity.update(cx, |this, cx| {
                        this.stream = value;
                        cx.notify();
                    });
                }
            },
        );

        SettingPage::new("Chat")
            .icon(IconName::Bot)
            .description("Mock values; M2 wires these to Ragrig.")
            .group(
                SettingGroup::new()
                    .title("Inference")
                    .item(
                        SettingItem::new("Agent model", model)
                            .description("Model used by the mock controller."),
                    )
                    .item(
                        SettingItem::new("Temperature", temperature)
                            .description("Higher values make answers less predictable."),
                    )
                    .item(
                        SettingItem::new("Stream responses", stream)
                            .description("Show tokens while the answer is generated."),
                    ),
            )
            .group(
                SettingGroup::new().title("Prompt").item(
                    SettingItem::new("System prompt", system_prompt)
                        .description("Sent before every question in M2.")
                        .layout(Axis::Vertical),
                ),
            )
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let page = self.page(cx);
        v_flex().size_full().bg(cx.theme().background).child(
            Settings::new("spike-settings")
                .sidebar_width(px(160.))
                .page(page),
        )
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
        "spike-settings"
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

// ---------------------------------------------------------------------------
// Root
// ---------------------------------------------------------------------------

struct SpikeApp {
    dock: Entity<DockArea>,
}

impl SpikeApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let chat = cx.new(|cx| ChatPanel::new(window, cx));
        let library = cx.new(LibraryPanel::new);
        let settings = cx.new(SettingsPanel::new);

        let (dock, _skin) = DockSkin::dock_area("spike-dock", Some(1), window, cx);
        dock.update(cx, |dock, cx| {
            dock.set_center(
                DockLayout::tabs().panel_view(panel_handle(chat), cx),
                window,
                cx,
            );
            dock.set_dock(
                DockPlacement::Left,
                DockLayout::tabs().panel_view(panel_handle(library), cx),
                window,
                cx,
            );
            dock.set_dock(
                DockPlacement::Right,
                DockLayout::tabs().panel_view(panel_handle(settings), cx),
                window,
                cx,
            );
            dock.set_dock_size(DockPlacement::Left, px(260.), window, cx);
            dock.set_dock_size(DockPlacement::Right, px(720.), window, cx);
        });

        Self { dock }
    }
}

impl Render for SpikeApp {
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
                    title: Some("ragrig-gpui — M0 spike".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };

            if let Err(error) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| SpikeApp::new(window, cx))
            }) {
                eprintln!("failed to open the spike window: {error:#}");
            }
        });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{TestAppContext, VisualTestContext};

    #[test]
    fn a_streamed_exchange_is_reduced_in_order() {
        let mut model = ChatModel::default();
        let index = model.begin("Explain the method");
        model.push_batch(index, "The ");
        model.push_batch(index, "method ");
        model.finish(index, false);
        model.push_batch(index, "late");

        assert_eq!(model.messages.len(), 2);
        assert_eq!(model.messages[0].role, Role::User);
        assert_eq!(model.messages[1].content, "The method ");
        assert!(!model.streaming);
        assert!(!model.messages[1].stopped);
    }

    #[test]
    fn a_stopped_exchange_keeps_its_partial_answer() {
        let mut model = ChatModel::default();
        let index = model.begin("Explain the method");
        model.push_batch(index, "Partial");

        model.finish(index, true);

        assert_eq!(model.messages[index].content, "Partial");
        assert!(model.messages[index].stopped);
    }

    fn mount(cx: &mut TestAppContext) -> (Entity<ChatPanel>, &mut VisualTestContext) {
        cx.update(ragrig_gpui::init);
        cx.add_window_view(ChatPanel::new)
    }

    fn focus_composer(panel: &Entity<ChatPanel>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .composer
                    .update(cx, |state, cx| state.focus(window, cx));
            });
        });
    }

    #[gpui_kit::test]
    fn enter_inserts_a_newline(cx: &mut TestAppContext) {
        let (panel, cx) = mount(cx);
        focus_composer(&panel, cx);

        cx.simulate_input("hello");
        cx.simulate_keystrokes("enter");

        cx.update(|_, cx| {
            assert_eq!(panel.read(cx).composer.read(cx).value(), "hello\n");
            assert!(panel.read(cx).model.read(cx).messages.is_empty());
        });
    }

    #[gpui_kit::test]
    #[ignore = "measurement: run with --ignored --nocapture"]
    fn measure_typing_latency(cx: &mut TestAppContext) {
        let (panel, cx) = mount(cx);
        focus_composer(&panel, cx);

        let text = "a".repeat(500);
        let started = std::time::Instant::now();
        cx.simulate_input(&text);
        let elapsed = started.elapsed();
        eprintln!(
            "typing {} chars took {elapsed:?} ({:?}/char)",
            text.len(),
            elapsed / text.len() as u32
        );

        cx.update(|_, cx| {
            assert_eq!(panel.read(cx).composer.read(cx).value().len(), text.len());
        });
    }

    #[gpui_kit::test]
    fn secondary_enter_submits(cx: &mut TestAppContext) {
        let (panel, cx) = mount(cx);
        focus_composer(&panel, cx);

        cx.simulate_input("Summarise the corpus");
        let shortcut = if cfg!(target_os = "macos") {
            "cmd-enter"
        } else {
            "ctrl-enter"
        };
        cx.simulate_keystrokes(shortcut);

        cx.update(|_, cx| {
            let panel = panel.read(cx);
            let model = panel.model.read(cx);
            assert_eq!(model.messages.len(), 2);
            assert_eq!(model.messages[0].content, "Summarise the corpus");
            assert!(model.streaming);
        });
    }

    fn next_event(rx: &Receiver<BridgeEvent>, deadline: std::time::Instant) -> BridgeEvent {
        while std::time::Instant::now() < deadline {
            if let Ok(event) = rx.try_recv() {
                return event;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("no bridge event arrived before the deadline");
    }

    #[test]
    fn the_tokio_worker_streams_and_cancels() {
        let (jobs, jobs_rx) = unbounded();
        spawn_worker(jobs_rx).expect("the worker thread should start");

        let (events, events_rx) = unbounded();
        let cancel = CancelFlag::new();
        jobs.try_send(StreamJob {
            prompt: "the bridge".into(),
            generation: 7,
            events: events.clone(),
            cancel: cancel.clone(),
        })
        .expect("the worker should accept a job");
        drop(events);

        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        let first = next_event(&events_rx, deadline);
        assert_eq!(first.generation, 7);
        match first.kind {
            BridgeKind::Token(text) => assert!(!text.is_empty()),
            other => panic!("expected a token batch, got {other:?}"),
        }

        cancel.cancel();
        loop {
            let event = next_event(&events_rx, deadline);
            if matches!(event.kind, BridgeKind::Cancelled) {
                break;
            }
            assert!(
                matches!(event.kind, BridgeKind::Token(_)),
                "a cancelled job must not report Done"
            );
        }
    }
}
