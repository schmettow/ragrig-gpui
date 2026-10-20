//! Ragrig GPUI demo: a configurable chat with library, sessions, references,
//! `@key` citations, and reference multi-select.
//!
//! This is the RAG half of Sciscraper without the editor. It uses the
//! reusable `ragrig-gpui` components and talks to a real Ragrig agent.
//!
//! ```text
//! RAGRIG_WORKSPACE=/path/to/workspace \
//! RAGRIG_CORPUS=/path/to/papers \
//! cargo run --example ragchat
//! ```
//!
//! `RAGRIG_WORKSPACE` selects the store/session folder (defaults to the
//! current directory). `RAGRIG_CORPUS` optionally indexes a folder at
//! startup. For DeepSeek, export `DEEPSEEK_API_KEY`; for Ollama, start
//! `ollama serve` and pull the configured chat and embedding models.

use std::path::PathBuf;

use gpui_kit::{
    AnyElement, App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle,
    Focusable, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window,
    WindowOptions,
    assets::IconName,
    base::dock::{Panel as BasePanel, PanelEvent},
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        dock::{
            DockArea, DockLayout, DockPlacement, DockSkin, Panel as ComponentPanel, panel_handle,
            register_panel,
        },
        h_flex,
        input::{Input, InputState},
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px, size,
};
use ragrig_gpui::prelude::*;

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

struct RagAppState {
    settings: Entity<RagSettings>,
    controller: Entity<RagController>,
    chat: Entity<RagChat>,
    corpus_path: Entity<InputState>,
    #[cfg(feature = "grobid")]
    selected: ReferenceSelection,
    status: SharedString,
}

impl RagAppState {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = cx.new(|_| {
            let mut settings = RagSettings::default();
            if let Ok(model) = std::env::var("RAGRIG_MODEL") {
                settings.ollama_model = model;
            }
            if let Ok(model) = std::env::var("RAGRIG_EMBED_MODEL") {
                settings.embed_model = model;
            }
            settings.refresh_profiles();
            settings
        });
        let setup = settings
            .read(cx)
            .to_setup()
            .expect("the default settings must be valid");
        let controller =
            cx.new(|cx| RagController::new(setup, cx).expect("the RAG worker thread must start"));
        let chat = cx.new(|cx| RagChat::new(window, cx, controller.clone()));
        chat.update(cx, |chat, cx| {
            chat.set_empty_state(
                EmptyState {
                    icon: IconName::Sparkles.into(),
                    title: "Ask your library".into(),
                    description: "Index a folder in the Library panel, then ask a question. \
                                  Answers stream with the documents they used. The library may \
                                  be empty: Ragrig still answers as a pure chat model."
                        .into(),
                },
                cx,
            );
        });

        let corpus_path = cx.new(|cx| InputState::new(window, cx).placeholder("/path/to/papers"));

        let mut state = Self {
            settings,
            controller,
            chat,
            corpus_path,
            #[cfg(feature = "grobid")]
            selected: ReferenceSelection::default(),
            status: "Ready. Configure the workspace and models, then index a folder.".into(),
        };

        let status_chat = state.chat.clone();
        cx.subscribe(
            &status_chat,
            |this, _, event: &RagChatEvent, cx| match event {
                RagChatEvent::Status(status) => this.set_status(status.clone(), cx),
                RagChatEvent::SourceClicked(source) => this.set_status(
                    format!("Source: {}{}", source.title, location_suffix(source)),
                    cx,
                ),
                RagChatEvent::Copied(_) => this.set_status("Message copied", cx),
            },
        )
        .detach();

        let status_controller = state.controller.clone();
        cx.subscribe(
            &status_controller,
            |this, _, event: &RagEvent, cx| match event {
                RagEvent::AgentReady(info) => this.set_status(
                    format!(
                        "{} ready · {} embeddings · workspace {}",
                        info.chat_model,
                        info.embed_model,
                        info.workspace.display()
                    ),
                    cx,
                ),
                RagEvent::AgentFailed { message } => {
                    this.set_status(format!("Agent error: {message}"), cx)
                }
                RagEvent::SyncProgress { progress, .. } => this.set_status(progress.summary(), cx),
                RagEvent::SyncFinished { summary, .. } => this.set_status(
                    format!(
                        "Indexed {} and {} from {}",
                        summary.documents, summary.chunks, summary.corpus
                    ),
                    cx,
                ),
                #[cfg(feature = "grobid")]
                RagEvent::ReferencesProgress { progress, .. } => {
                    this.set_status(progress.summary(), cx)
                }
                #[cfg(feature = "grobid")]
                RagEvent::ReferencesFinished { report, .. } => {
                    this.set_status(report.summary(), cx)
                }
                #[cfg(feature = "grobid")]
                RagEvent::ReferencesFailed { message, .. } => this.set_status(message.clone(), cx),
                #[cfg(feature = "grobid")]
                RagEvent::ReferencesLoaded { entries, .. } => {
                    this.selected.retain_existing(entries);
                    this.set_status(format!("{} references", entries.len()), cx)
                }
                #[cfg(feature = "grobid")]
                RagEvent::Citations { resolved, .. } if !resolved.is_empty() => {
                    let count = resolved.len();
                    let noun = if count == 1 { "paper" } else { "papers" };
                    this.set_status(format!("Attached {count} cited {noun}"), cx)
                }
                _ => {}
            },
        )
        .detach();

        if let Some(folder) = std::env::var_os("RAGRIG_CORPUS") {
            let folder = PathBuf::from(folder);
            state.index_folder(folder, cx);
        }

        #[cfg(feature = "grobid")]
        {
            state.refresh_references(cx);
            if std::env::var_os("RAGRIG_GROBID").is_some() {
                state.extract_references(cx);
            }
        }

        if let Ok(prompt) = std::env::var("RAGRIG_ASK")
            && !prompt.trim().is_empty()
        {
            let chat = state.chat.clone();
            cx.defer(move |cx| {
                chat.update(cx, |chat, cx| chat.ask(prompt, cx));
            });
        }

        state
    }

    fn set_status(&mut self, status: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.status = status.into();
        cx.notify();
    }

    fn index_folder(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        if !folder.is_dir() {
            self.set_status(format!("'{}' is not a folder", folder.display()), cx);
            return;
        }
        let name = folder
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("corpus")
            .to_owned();
        self.settings.update(cx, |settings, cx| {
            settings.add_corpus(&name, &folder);
            cx.notify();
        });
        match self.controller.update(cx, |controller, cx| {
            controller.sync_corpus(name, folder, cx)
        }) {
            Ok(_) => self.set_status("Indexing…", cx),
            Err(message) => self.set_status(message, cx),
        }
    }

    fn index_from_input(&mut self, cx: &mut Context<Self>) {
        let text = self.corpus_path.read(cx).value().trim().to_owned();
        if text.is_empty() {
            self.set_status("enter a folder path first", cx);
            return;
        }
        self.index_folder(PathBuf::from(text), cx);
    }

    #[cfg(feature = "grobid")]
    fn reference_folder(&self, cx: &App) -> PathBuf {
        let input = self.corpus_path.read(cx).value().trim().to_owned();
        if !input.is_empty() {
            let path = PathBuf::from(input);
            if path.is_dir() {
                return path;
            }
        }
        for corpus in &self.settings.read(cx).corpus_dirs {
            if let Some((_, path)) = corpus.split_once('=') {
                let path = PathBuf::from(path.trim());
                if path.is_dir() {
                    return path;
                }
            }
        }
        let workspace = PathBuf::from(self.settings.read(cx).workspace.trim());
        if workspace.is_dir() {
            workspace
        } else {
            PathBuf::from(".")
        }
    }

    #[cfg(feature = "grobid")]
    fn extract_references(&mut self, cx: &mut Context<Self>) {
        let folder = self.reference_folder(cx);
        let options = self.settings.read(cx).reference_options();
        match self.controller.update(cx, |controller, cx| {
            controller.extract_references(folder, options, cx)
        }) {
            Ok(_) => self.set_status("Parsing references with GROBID…", cx),
            Err(message) => self.set_status(message, cx),
        }
    }

    #[cfg(feature = "grobid")]
    fn refresh_references(&mut self, cx: &mut Context<Self>) {
        let folder = self.reference_folder(cx);
        self.controller.update(cx, |controller, cx| {
            controller.refresh_references(folder, cx)
        });
    }
}

fn location_suffix(source: &SourceRef) -> String {
    match &source.location {
        Some(location) => format!(" ({location})"),
        None => String::new(),
    }
}

fn observe_state<T: 'static>(observed: &Entity<RagAppState>, cx: &mut Context<T>) {
    cx.observe(observed, |_, _, cx| cx.notify()).detach();
    let (settings, controller, chat) = {
        let state = observed.read(cx);
        (
            state.settings.clone(),
            state.controller.clone(),
            state.chat.clone(),
        )
    };
    cx.observe(&settings, |_, _, cx| cx.notify()).detach();
    cx.observe(&controller, |_, _, cx| cx.notify()).detach();
    cx.observe(&chat, |_, _, cx| cx.notify()).detach();
}

// ---------------------------------------------------------------------------
// Chat panel
// ---------------------------------------------------------------------------

struct ChatPanel {
    state: Entity<RagAppState>,
    focus_handle: FocusHandle,
}

impl ChatPanel {
    fn new(state: Entity<RagAppState>, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for ChatPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (controller, chat, status) = {
            let state = self.state.read(cx);
            (
                state.controller.clone(),
                state.chat.clone(),
                state.status.clone(),
            )
        };
        let controller_read = controller.read(cx);
        let components = controller_read.agent_components();
        let status_line = controller_read.status_line();
        let error = controller_read.error().map(str::to_owned);
        let sessions = controller_read.sessions().to_vec();
        let current = controller_read.current_session().map(str::to_owned);
        let has_agent = controller_read.agent().is_some();

        let agent_state = self.state.clone();
        let agent_bar =
            AgentBar::new("ragchat-agent", components).on_event(move |event, _, cx| match event {
                AgentBarEvent::Configure(id) => {
                    agent_state.update(cx, |state, cx| {
                        state.set_status(format!("Configure {id} in the Settings tab"), cx)
                    });
                }
                AgentBarEvent::Toggle {
                    component, enabled, ..
                } => {
                    agent_state.update(cx, |state, cx| {
                        state.settings.update(cx, |settings, cx| {
                            match component.as_str() {
                                "memory" => {
                                    settings.memory = if enabled {
                                        MemoryChoice::Rewrite
                                    } else {
                                        MemoryChoice::Off
                                    };
                                }
                                "hybrid" => settings.hybrid = enabled,
                                _ => return,
                            }
                            cx.notify();
                        });
                        let setup = state.settings.read(cx).to_setup();
                        match setup {
                            Ok(setup) => {
                                state
                                    .controller
                                    .update(cx, |controller, cx| controller.apply(setup, cx));
                                state.set_status(
                                    format!(
                                        "{} {}",
                                        component,
                                        if enabled { "enabled" } else { "disabled" }
                                    ),
                                    cx,
                                );
                            }
                            Err(error) => state.set_status(error, cx),
                        }
                    });
                }
                AgentBarEvent::OpenSettings => {
                    agent_state.update(cx, |state, cx| {
                        state.set_status("Settings are in the right dock", cx)
                    });
                }
            });

        let picker_controller = controller.clone();
        let mut picker = SessionPicker::new("ragchat-picker", sessions);
        if let Some(current) = current {
            picker = picker.current(current);
        }
        let picker = picker.on_event(move |event, _, cx| match event {
            SessionPickerEvent::New => {
                picker_controller.update(cx, |controller, cx| controller.new_session(cx));
            }
            SessionPickerEvent::Load(id) => {
                picker_controller.update(cx, |controller, cx| controller.load_session(id, cx));
            }
            SessionPickerEvent::Refresh => {
                picker_controller.update(cx, |controller, cx| controller.refresh_sessions(cx));
            }
        });

        let banner = if let Some(error) = error {
            Some(
                div()
                    .mx_3()
                    .mt_2()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().danger)
                    .bg(cx.theme().danger.opacity(0.08))
                    .text_sm()
                    .child(format!(
                        "{error}\nOpen Settings, fix the workspace or models, then press Apply. \
                         DeepSeek reads DEEPSEEK_API_KEY from the environment."
                    )),
            )
        } else if !has_agent {
            Some(
                div()
                    .mx_3()
                    .mt_2()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .border_1()
                    .border_color(cx.theme().border)
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Building the agent. Set the workspace and models in Settings."),
            )
        } else {
            None
        };

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
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(status_line),
                    ),
            )
            .when_some(banner, |this, banner| this.child(banner))
            .child(div().flex_1().min_h(px(0.)).child(chat))
            .child(
                div()
                    .px_3()
                    .py_1()
                    .border_t_1()
                    .border_color(cx.theme().border)
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
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
        "ragchat-chat"
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
// Library panel
// ---------------------------------------------------------------------------

struct LibraryPanel {
    state: Entity<RagAppState>,
    focus_handle: FocusHandle,
}

impl LibraryPanel {
    fn new(state: Entity<RagAppState>, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for LibraryPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (controller, settings, corpus_path) = {
            let state = self.state.read(cx);
            (
                state.controller.clone(),
                state.settings.clone(),
                state.corpus_path.clone(),
            )
        };
        let (documents, chunks, progress, ingesting, busy) = {
            let controller = controller.read(cx);
            (
                controller.library().documents.clone(),
                controller.library().chunks,
                controller.ingest_progress().cloned(),
                controller.is_ingesting(),
                controller.is_busy(),
            )
        };
        let corpora = settings.read(cx).corpus_dirs.clone();
        #[cfg_attr(not(feature = "grobid"), allow(unused_mut))]
        let mut selection_footer: Option<AnyElement> = None;

        let index_state = self.state.clone();
        let index = Button::new("ragchat-index-body")
            .label("Index")
            .icon(IconName::FolderPlus)
            .small()
            .primary()
            .disabled(ingesting || busy)
            .tooltip("Index the folder incrementally")
            .on_click(move |_, _, cx| {
                index_state.update(cx, |state, cx| state.index_from_input(cx));
            });
        let cancel = {
            let controller = controller.clone();
            Button::new("ragchat-index-cancel")
                .icon(IconName::Square)
                .small()
                .ghost()
                .tooltip("Stop indexing")
                .disabled(!ingesting)
                .on_click(move |_, _, cx| {
                    controller.update(cx, |controller, cx| controller.stop_sync(cx));
                })
        };
        let refresh = {
            let controller = controller.clone();
            Button::new("ragchat-library-refresh")
                .icon(IconName::RefreshCw)
                .small()
                .ghost()
                .tooltip("Refresh library")
                .on_click(move |_, _, cx| {
                    controller.update(cx, |controller, cx| controller.refresh_library(cx));
                })
        };

        let mut body = v_flex().gap_2();
        if corpora.is_empty() {
            body = body.child(muted(
                "No corpora yet. Enter a folder above and index it.",
                cx,
            ));
        } else {
            for (index, corpus) in corpora.iter().enumerate() {
                body = body.child(
                    h_flex()
                        .gap_2()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(IconName::Folder),
                        )
                        .child(
                            div().text_sm().child(
                                corpus
                                    .split_once('=')
                                    .map_or(corpus.as_str(), |(_, path)| path)
                                    .to_owned(),
                            ),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("#{index}")),
                        ),
                );
            }
        }

        if let Some(progress) = progress {
            body = body.child(
                v_flex()
                    .gap_1()
                    .p_2()
                    .rounded(cx.theme().radius)
                    .bg(cx.theme().muted.opacity(0.35))
                    .child(
                        div()
                            .text_sm()
                            .child(format!("Indexing {}", progress.summary())),
                    )
                    .when_some(progress.fraction(), |card, fraction| {
                        card.child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{:.0}% embedded", fraction * 100.0)),
                        )
                    }),
            );
        }

        let mut documents_body = v_flex().gap_2().child(
            div()
                .mt_2()
                .text_sm()
                .child(format!("Documents · {chunks} chunks")),
        );
        if documents.is_empty() {
            documents_body = documents_body.child(muted(
                "The store is empty. Chat still works without retrieval.",
                cx,
            ));
        } else {
            for (index, document) in documents.into_iter().enumerate() {
                let attach = {
                    let chat = self.state.read(cx).chat.clone();
                    let path = document.path.clone();
                    let name = document.name.clone();
                    let id = ElementId::Name(format!("ragchat-attach-{index}").into());
                    let mut button = Button::new(id)
                        .icon(IconName::Paperclip)
                        .xsmall()
                        .ghost()
                        .tooltip("Attach to next question");
                    if let Some(path) = path {
                        let attach_name = name.clone();
                        button = button.on_click(move |_, _, cx| {
                            let path = path.clone();
                            let name = attach_name.clone();
                            chat.update(cx, |chat, cx| chat.attach_file(name, path, cx));
                        });
                    } else {
                        button = button.disabled(true).tooltip("Path not found");
                    }
                    button
                };
                let detail = format!(
                    "{} · {} · {}",
                    document.corpus, document.parser, document.chunker
                );
                documents_body = documents_body.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .border_1()
                        .border_color(cx.theme().border)
                        .child(
                            div()
                                .text_color(cx.theme().muted_foreground)
                                .child(IconName::FileText),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap_1()
                                .child(div().min_w_0().truncate().text_sm().child(document.name))
                                .child(
                                    div()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(cx.theme().muted_foreground)
                                        .child(detail),
                                ),
                        )
                        .child(attach),
                );
            }
        }

        #[cfg(feature = "grobid")]
        {
            let controller = self.state.read(cx).controller.clone();
            let (references, bibliography, report, reference_progress, extracting) = {
                let controller = controller.read(cx);
                (
                    controller.references().to_vec(),
                    controller
                        .reference_bibliography()
                        .map(|path| path.to_path_buf()),
                    controller.reference_report().cloned(),
                    controller.reference_progress().cloned(),
                    controller.is_extracting_references(),
                )
            };

            let selected_keys = self.state.read(cx).selected.clone();
            let selected_entries: Vec<ReferenceEntry> = selected_keys
                .entries(&references)
                .into_iter()
                .cloned()
                .collect();

            let parse_state = self.state.clone();
            let parse = Button::new("ragchat-grobid-parse")
                .label("GROBID")
                .icon(IconName::BookOpen)
                .small()
                .outline()
                .disabled(extracting)
                .tooltip("Parse the folder's PDFs and merge references")
                .on_click(move |_, _, cx| {
                    parse_state.update(cx, |state, cx| state.extract_references(cx));
                });
            let refresh_references = {
                let controller = controller.clone();
                let folder = self.state.read(cx).reference_folder(cx);
                Button::new("ragchat-references-refresh")
                    .icon(IconName::RefreshCw)
                    .small()
                    .ghost()
                    .tooltip("Reload the bibliography")
                    .on_click(move |_, _, cx| {
                        controller.update(cx, |controller, cx| {
                            controller.refresh_references(folder.clone(), cx)
                        });
                    })
            };

            body = body.child(
                h_flex()
                    .mt_2()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .child(format!("References · {}", references.len())),
                    )
                    .child(h_flex().gap_1().child(parse).child(refresh_references)),
            );

            if let Some(progress) = reference_progress {
                body = body.child(
                    v_flex()
                        .gap_1()
                        .p_2()
                        .rounded(cx.theme().radius)
                        .bg(cx.theme().muted.opacity(0.35))
                        .child(div().text_sm().child(progress.summary())),
                );
            }
            if let Some(report) = report {
                body = body.child(muted(format!("Last run: {}", report.summary()), cx));
                for failure in report.failed {
                    body = body.child(muted(failure, cx));
                }
            }
            if let Some(path) = bibliography {
                body = body.child(muted(format!("Bibliography: {}", path.display()), cx));
            }
            if references.is_empty() {
                body = body.child(muted(
                    "No references yet. Press GROBID to parse this folder's PDFs.",
                    cx,
                ));
            } else {
                for (index, entry) in references.into_iter().enumerate() {
                    let key = entry.key.clone();
                    let is_selected = selected_keys.contains(&key);
                    let copy_state = self.state.clone();
                    let select_state = self.state.clone();
                    body = body.child(
                        ReferenceRow::new(index, entry)
                            .selected(is_selected)
                            .on_copy(move |copy, _, cx| {
                                copy_state.update(cx, |state, cx| match copy {
                                    ReferenceCopy::Key(key) => {
                                        state.set_status(format!("Copied @{key}"), cx)
                                    }
                                    ReferenceCopy::Path(path) => {
                                        state.set_status(format!("Copied {}", path.display()), cx)
                                    }
                                });
                            })
                            .on_select(move |_, cx| {
                                let key = key.clone();
                                select_state.update(cx, |state, cx| {
                                    state.selected.toggle(key);
                                    cx.notify();
                                });
                            }),
                    );
                }
            }

            if !selected_entries.is_empty() {
                let action_state = self.state.clone();
                let action_chat = self.state.read(cx).chat.clone();
                selection_footer = Some(
                    ReferenceSelectionBar::new(selected_entries)
                        .on_action(move |action, entries, _, cx| match action {
                            ReferenceAction::Insert => action_state.update(cx, |state, cx| {
                                state.set_status("Insert is available in the writing host", cx);
                            }),
                            ReferenceAction::CopyKeys => action_state.update(cx, |state, cx| {
                                let count = entries.len();
                                state.set_status(format!("Copied {count} @keys"), cx);
                            }),
                            ReferenceAction::CopyPaths => action_state.update(cx, |state, cx| {
                                let count =
                                    entries.iter().filter(|entry| entry.path.is_some()).count();
                                let noun = if count == 1 { "path" } else { "paths" };
                                state.set_status(format!("Copied {count} {noun}"), cx);
                            }),
                            ReferenceAction::Attach => {
                                let mut attached = 0;
                                for entry in entries {
                                    let Some(path) = &entry.path else {
                                        continue;
                                    };
                                    let label = format!("@{}", entry.key);
                                    action_chat.update(cx, |chat, cx| {
                                        chat.attach_file(label.clone(), path.clone(), cx);
                                    });
                                    attached += 1;
                                }
                                action_state.update(cx, |state, cx| {
                                    state.selected.clear();
                                    let noun = if attached == 1 {
                                        "reference"
                                    } else {
                                        "references"
                                    };
                                    state.set_status(
                                        format!("Attached {attached} {noun} to the next question"),
                                        cx,
                                    );
                                });
                            }
                            ReferenceAction::Clear => action_state.update(cx, |state, cx| {
                                state.selected.clear();
                                cx.notify();
                            }),
                        })
                        .into_any_element(),
                );
            }
        }

        body = body.child(documents_body);

        SidePanel::new("ragchat-library", "Library")
            .subtitle("Corpora and documents")
            .action(cancel)
            .action(refresh)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .child(Input::new(&corpus_path).small().bordered(true)),
                    )
                    .child(index),
            )
            .child(body)
            .footer(selection_footer.unwrap_or_else(|| {
                muted("Indexing is incremental: unchanged files are skipped.", cx)
            }))
    }
}

fn muted(text: impl Into<SharedString>, cx: &App) -> AnyElement {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
        .into_any_element()
}

impl EventEmitter<PanelEvent> for LibraryPanel {}

impl Focusable for LibraryPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for LibraryPanel {
    fn panel_name(&self) -> &'static str {
        "ragchat-library"
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

// ---------------------------------------------------------------------------
// Sessions panel
// ---------------------------------------------------------------------------

struct SessionsPanel {
    state: Entity<RagAppState>,
    focus_handle: FocusHandle,
}

impl SessionsPanel {
    fn new(state: Entity<RagAppState>, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for SessionsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controller = self.state.read(cx).controller.clone();
        let (sessions, current) = {
            let controller = controller.read(cx);
            (
                controller.sessions().to_vec(),
                controller.current_session().map(str::to_owned),
            )
        };

        let new_controller = controller.clone();
        let new = Button::new("ragchat-new-session")
            .label("New")
            .icon(IconName::Plus)
            .small()
            .outline()
            .on_click(move |_, _, cx| {
                new_controller.update(cx, |controller, cx| controller.new_session(cx));
            });
        let refresh_controller = controller.clone();
        let refresh = Button::new("ragchat-refresh-sessions")
            .label("Refresh")
            .icon(IconName::RefreshCw)
            .small()
            .ghost()
            .on_click(move |_, _, cx| {
                refresh_controller.update(cx, |controller, cx| controller.refresh_sessions(cx));
            });

        let mut list = v_flex().gap_2();
        if sessions.is_empty() {
            list = list.child(muted("No saved conversations yet.", cx));
        }
        for (index, session) in sessions.into_iter().enumerate() {
            let is_current = current.as_deref() == Some(session.id.as_str());
            let load_controller = controller.clone();
            let id = session.id.clone();
            let button = Button::new(ElementId::Name(format!("ragchat-session-{index}").into()))
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
                    load_controller
                        .update(cx, |controller, cx| controller.load_session(id.clone(), cx));
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

        SidePanel::new("ragchat-session-list", "Sessions")
            .subtitle("Saved conversations")
            .action(new)
            .action(refresh)
            .child(list)
            .footer(muted("Sessions are JSON files under .ragrig/sessions.", cx))
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
        "ragchat-sessions"
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
// Settings and components panels
// ---------------------------------------------------------------------------

struct SettingsPanel {
    state: Entity<RagAppState>,
    focus_handle: FocusHandle,
}

impl SettingsPanel {
    fn new(state: Entity<RagAppState>, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for SettingsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (settings, controller) = {
            let state = self.state.read(cx);
            (state.settings.clone(), state.controller.clone())
        };
        div()
            .size_full()
            .bg(cx.theme().background)
            .child(rag_settings(
                ElementId::Name("ragchat-settings".into()),
                settings,
                controller,
                cx,
            ))
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
        "ragchat-settings-panel"
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
    state: Entity<RagAppState>,
    focus_handle: FocusHandle,
}

impl ComponentsPanel {
    fn new(state: Entity<RagAppState>, cx: &mut Context<Self>) -> Self {
        observe_state(&state, cx);
        Self {
            state,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl Render for ComponentsPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let controller = self.state.read(cx).controller.clone();
        let (components, status) = {
            let controller = controller.read(cx);
            (controller.agent_components(), controller.status_line())
        };

        let mut cards = v_flex().gap_2();
        for component in components {
            cards = cards.child(component_card(component, cx));
        }

        SidePanel::new("ragchat-components", "Components")
            .subtitle("Ragrig pipeline")
            .child(cards)
            .footer(muted(status, cx))
    }
}

fn component_card(component: AgentComponent, cx: &App) -> AnyElement {
    let theme = cx.theme();
    let color = match component.status {
        ComponentStatus::Inactive => theme.muted_foreground,
        ComponentStatus::Ready => theme.success,
        ComponentStatus::Busy => theme.info,
        ComponentStatus::Error => theme.danger,
    };
    h_flex()
        .gap_2()
        .items_center()
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
                .flex_1()
                .gap_1()
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
            div()
                .text_xs()
                .text_color(color)
                .child(component.status.label()),
        )
        .into_any_element()
}

impl EventEmitter<PanelEvent> for ComponentsPanel {}

impl Focusable for ComponentsPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl BasePanel for ComponentsPanel {
    fn panel_name(&self) -> &'static str {
        "ragchat-components-panel"
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

struct RagApp {
    state: Entity<RagAppState>,
    dock: Entity<DockArea>,
}

impl RagApp {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let state = cx.new(|cx| RagAppState::new(window, cx));

        {
            let state = state.clone();
            register_panel(cx, "ragchat-chat", move |_, _, cx| {
                panel_handle(cx.new(|cx| ChatPanel::new(state.clone(), cx)))
            });
        }
        {
            let state = state.clone();
            register_panel(cx, "ragchat-library", move |_, _, cx| {
                panel_handle(cx.new(|cx| LibraryPanel::new(state.clone(), cx)))
            });
        }
        {
            let state = state.clone();
            register_panel(cx, "ragchat-sessions", move |_, _, cx| {
                panel_handle(cx.new(|cx| SessionsPanel::new(state.clone(), cx)))
            });
        }
        {
            let state = state.clone();
            register_panel(cx, "ragchat-settings", move |_, _, cx| {
                panel_handle(cx.new(|cx| SettingsPanel::new(state.clone(), cx)))
            });
        }
        {
            let state = state.clone();
            register_panel(cx, "ragchat-components", move |_, _, cx| {
                panel_handle(cx.new(|cx| ComponentsPanel::new(state.clone(), cx)))
            });
        }

        let chat = cx.new(|cx| ChatPanel::new(state.clone(), cx));
        let library = cx.new(|cx| LibraryPanel::new(state.clone(), cx));
        let sessions = cx.new(|cx| SessionsPanel::new(state.clone(), cx));
        let settings = cx.new(|cx| SettingsPanel::new(state.clone(), cx));
        let components = cx.new(|cx| ComponentsPanel::new(state.clone(), cx));

        let (dock, _skin) = DockSkin::dock_area("ragchat-dock", Some(1), window, cx);
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
            dock.set_dock_size(DockPlacement::Left, px(320.), window, cx);
            dock.set_dock_size(DockPlacement::Right, px(700.), window, cx);
        });

        Self { state, dock }
    }
}

impl Render for RagApp {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _ = &self.state;
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

            let restore = gpui_kit::Bounds::centered(None, size(px(1680.), px(1000.)), cx);
            let options = WindowOptions {
                window_bounds: Some(gpui_kit::WindowBounds::Windowed(restore)),
                titlebar: Some(gpui_kit::TitlebarOptions {
                    title: Some("ragrig-gpui — RAG chat".into()),
                    ..Default::default()
                }),
                ..Default::default()
            };

            if let Err(error) = gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| RagApp::new(window, cx))
            }) {
                eprintln!("failed to open the ragchat window: {error:#}");
            }
        });
}
