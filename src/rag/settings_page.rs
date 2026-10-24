//! The Ragrig settings pages.
//!
//! Hosts can take the complete page, or split it by concern: [`rag_model_page`]
//! contains the model, retrieval, parsing, and memory groups, while
//! [`rag_references_group`] can be added to a host's own library page.
//!
//! Rows are built with [`SettingRow`] rather than [`SettingItem::new`] so a
//! field that differs from the applied configuration can be marked in bold
//! violet. Pass a [`Changes`] closure to report those differences; a host that
//! does not track them can pass [`no_changes`].

use std::{path::Path, rc::Rc};

use gpui_kit::{
    AnyElement, App, ElementId, Entity, FontWeight, IntoElement, ParentElement as _, SharedString,
    Styled as _, Window,
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        setting::{
            AnySettingField, NumberFieldOptions, SettingField, SettingGroup, SettingItem,
            SettingPage, Settings,
        },
    },
    div,
    prelude::FluentBuilder as _,
};

use super::{
    ChatProvider, MemoryChoice, PdfBackend, RagChanges, RagController, RagField, RagSection,
    RagSettings,
};
use crate::{
    SettingRow, dirty_color, dirty_header, dropdown_control, number_control, path_control,
    path_setting, switch_control, text_control, token_control,
};

/// Reports which settings differ from the configuration applied to the worker.
pub type Changes = Rc<dyn Fn(&App) -> RagChanges>;

/// A [`Changes`] closure for hosts that do not track pending edits.
pub fn no_changes() -> Changes {
    Rc::new(|_| RagChanges::default())
}

/// Opens the effective prompt in the host editor.
///
/// `None` means no file is selected and the host should show its built-in
/// prompt.
pub type OpenPrompt = Rc<dyn Fn(Option<&Path>, &mut Window, &mut App)>;

/// Writes the host's built-in prompt into the workspace and opens it.
///
/// Returns the new absolute path on success.
pub type ExportPrompt = Rc<dyn Fn(&mut Window, &mut App) -> Result<String, String>>;

/// Host actions for the system-prompt file row.
///
/// The generic settings page owns the path field, but only the host can open a
/// file in its editor or write its compiled-in prompt into the workspace.
#[derive(Clone)]
pub struct PromptActions {
    /// File name of the host's compiled-in prompt, used in labels and help.
    pub built_in_name: &'static str,
    /// Open the prompt in the host editor; `None` is the built-in prompt.
    pub open: OpenPrompt,
    /// Write the built-in prompt into the workspace, select it, and open it.
    pub export: ExportPrompt,
}

/// Build the complete Ragrig settings page.
///
/// The Apply row validates [`RagSettings`] and asks the [`RagController`] to
/// rebuild the agent. Profile actions are filesystem-only; they do not touch
/// the running agent until Apply is pressed.
pub fn rag_settings_page(
    settings: Entity<RagSettings>,
    controller: Entity<RagController>,
    cx: &App,
) -> SettingPage {
    let changes = no_changes();
    let now = RagChanges::default();
    let mut page = SettingPage::new("RAG")
        .description(
            "Ragrig workspace, models, retrieval, and memory. Press Apply to rebuild the agent.",
        )
        .default_open(true)
        .group(workspace_group(&settings))
        .group(chat_group(&settings, &changes, &now, None))
        .group(retrieval_group(&settings, &changes, &now))
        .group(parsing_group(&settings, &changes, &now))
        .group(memory_group(&settings, &changes, &now))
        .group(rag_references_group(&settings, &changes));
    page = page
        .group(profiles_group(&settings, cx))
        .group(apply_group(&settings, &controller));
    page
}

/// The model, retrieval, parsing, and memory groups.
///
/// This is the RAG page for a host that keeps workspace, bibliography,
/// profile management, and reference ingestion on its own pages. Pass
/// [`PromptActions`] to add the open and export buttons beside the prompt file
/// field; `None` renders the path field alone.
pub fn rag_model_page(
    settings: Entity<RagSettings>,
    changes: Changes,
    prompt: Option<PromptActions>,
    cx: &App,
) -> SettingPage {
    let now = changes(cx);
    let page_dirty = [
        RagSection::Chat,
        RagSection::Retrieval,
        RagSection::Parsing,
        RagSection::Memory,
    ]
    .iter()
    .any(|section| now.section(*section));
    let page = SettingPage::new(group_title("RAG", page_dirty))
        .description(
            "Models, retrieval, parsing, and memory. Press Apply on Overview to rebuild the agent.",
        )
        .default_open(true);
    let page = if page_dirty {
        page.header_style(&dirty_header(cx))
    } else {
        page
    };
    page.group(chat_group(&settings, &changes, &now, prompt.as_ref()))
        .group(retrieval_group(&settings, &changes, &now))
        .group(parsing_group(&settings, &changes, &now))
        .group(memory_group(&settings, &changes, &now))
}

/// The workspace and bibliography group for hosts that use Ragrig's page.
fn workspace_group(settings: &Entity<RagSettings>) -> SettingGroup {
    let settings_for_workspace = settings.clone();
    let workspace = path_setting(
        "rag-workspace",
        "Choose the Ragrig workspace",
        true,
        false,
        {
            let settings = settings.clone();
            move |cx| settings.read(cx).workspace.clone()
        },
        move |value, cx| {
            settings_for_workspace.update(cx, |settings, cx| {
                settings.workspace = value;
                cx.notify();
            });
        },
    );
    let settings_for_bibliography = settings.clone();
    let bibliography = path_setting(
        "rag-bibliography",
        "Choose the bibliography file",
        false,
        true,
        {
            let settings = settings.clone();
            move |cx| settings.read(cx).bibliography.clone()
        },
        move |value, cx| {
            settings_for_bibliography.update(cx, |settings, cx| {
                settings.bibliography = value;
                cx.notify();
            });
        },
    );

    SettingGroup::new()
        .title("Workspace")
        .item(
            SettingItem::new("Workspace", workspace)
                .description("Store, sessions, and profiles live under this folder (.ragrig)."),
        )
        .item(SettingItem::new("Bibliography", bibliography).description(
            "The .bib the library reads and GROBID writes. Empty uses .ragrig/references.bib.",
        ))
}

fn chat_group(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    now: &RagChanges,
    prompt: Option<&PromptActions>,
) -> SettingGroup {
    let provider = dropdown_row(
        settings,
        changes,
        spec("rag-provider", RagField::Provider, "Provider"),
        move |_| provider_options(),
        |settings| settings.provider.label().to_owned(),
        |settings, value| {
            settings.provider = ChatProvider::from_label(&value).unwrap_or(settings.provider);
        },
    )
    .description(format!(
        "Cloud keys: {}. A missing key is reported when the agent starts.",
        ChatProvider::options()
            .iter()
            .filter_map(|(provider, _)| provider.env_var())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let model = text_row(
        settings,
        changes,
        spec("rag-model", RagField::Model, "Model"),
        |settings| settings.model().to_owned(),
        |settings, value| settings.set_model(value),
    )
    .description("Used by the selected provider; each provider keeps its own model.");
    let stream = switch_row(
        settings,
        changes,
        spec("rag-stream", RagField::Stream, "Stream responses"),
        |settings| settings.stream,
        |settings, value| settings.stream = value,
    );
    let temperature = number_row(
        settings,
        changes,
        spec("rag-temperature", RagField::Temperature, "Temperature"),
        NumberFieldOptions {
            min: 0.0,
            max: 2.0,
            step: 0.1,
        },
        |settings| settings.temperature,
        |settings, value| settings.temperature = value.clamp(0.0, 2.0),
    )
    .description("0 is deterministic; 2 is very loose.");
    let max_tokens = token_row(
        settings,
        changes,
        spec("rag-max-tokens", RagField::MaxTokens, "Maximum tokens"),
        9,
        20,
        |settings| settings.max_tokens,
        |settings, value| settings.max_tokens = value,
    );
    let context_tokens = token_row(
        settings,
        changes,
        spec(
            "rag-context-tokens",
            RagField::ContextTokens,
            "Context tokens",
        ),
        9,
        20,
        |settings| settings.context_tokens,
        |settings, value| settings.context_tokens = value,
    );
    let prompt_path = path_control(
        "rag-system-prompt-path",
        "Choose a Markdown prompt file",
        false,
        true,
        {
            let settings = settings.clone();
            move |cx| settings.read(cx).system_prompt_path.clone()
        },
        {
            let settings = settings.clone();
            move |value, cx| {
                settings.update(cx, |settings, cx| {
                    settings.system_prompt_path = value;
                    cx.notify();
                });
            }
        },
    );
    let built_in_name = prompt.map_or("built-in prompt", |prompt| prompt.built_in_name);
    let system_prompt = SettingRow::new("System prompt", prompt_path)
        .description(format!(
            "Empty uses the {built_in_name}. A selected Markdown file overrides it; Apply rebuilds the agent."
        ))
        .dirty({
            let changes = changes.clone();
            move |cx| changes(cx).contains(RagField::SystemPrompt)
        })
        .into_item();

    let mut group = SettingGroup::new()
        .title(group_title("Chat", now.section(RagSection::Chat)))
        .item(provider.into_item())
        .item(model.into_item())
        .item(stream.into_item())
        .item(temperature.into_item())
        .item(max_tokens.into_item())
        .item(context_tokens.into_item())
        .item(system_prompt);
    if let Some(prompt) = prompt {
        let settings = settings.clone();
        let prompt = prompt.clone();
        group = group.item(SettingItem::render(move |_, window, cx| {
            prompt_buttons(settings.clone(), prompt.clone(), window, cx)
        }));
    }

    mark_group(group, now, RagSection::Chat)
}

fn prompt_buttons(
    settings: Entity<RagSettings>,
    actions: PromptActions,
    _: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let built_in = settings.read(cx).built_in_prompt.clone();
    let path = settings.read(cx).system_prompt_path.clone();
    let status = settings.read(cx).status.clone();
    let uses_built_in =
        path.trim().is_empty() && settings.read(cx).system_prompt.trim() == built_in.trim();

    let open = {
        let settings = settings.clone();
        let actions = actions.clone();
        Button::new("rag-prompt-open")
            .label("Open prompt")
            .icon(gpui_kit::assets::IconName::FolderOpen)
            .small()
            .outline()
            .on_click(move |_, window, cx| {
                let path = settings.read(cx).prompt_path();
                (actions.open)(path.as_deref(), window, cx);
            })
    };
    let export = {
        let settings = settings.clone();
        let actions = actions.clone();
        let label = format!("Export {}", actions.built_in_name);
        let built_in_name = actions.built_in_name;
        Button::new("rag-prompt-export")
            .label(label)
            .icon(gpui_kit::assets::IconName::Save)
            .small()
            .outline()
            .on_click(move |_, window, cx| match (actions.export)(window, cx) {
                Ok(path) => settings.update(cx, |settings, cx| {
                    settings.system_prompt_path = path;
                    settings.status = Some(format!(
                        "Using {built_in_name} — edit it, then press Apply to rebuild the agent"
                    ));
                    cx.notify();
                }),
                Err(message) => settings.update(cx, |settings, cx| {
                    settings.status = Some(message);
                    cx.notify();
                }),
            })
    };
    let reset = (!uses_built_in).then(|| {
        let settings = settings.clone();
        Button::new("rag-prompt-built-in")
            .label("Use built-in")
            .icon(gpui_kit::assets::IconName::RefreshCw)
            .small()
            .ghost()
            .on_click(move |_, _, cx| {
                settings.update(cx, |settings, cx| {
                    settings.use_built_in_prompt();
                    cx.notify();
                });
            })
    });

    h_flex()
        .gap_2()
        .items_center()
        .child(open)
        .child(export)
        .when_some(reset, |row, reset| row.child(reset))
        .when_some(status, |row, status| {
            row.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
        })
        .into_any_element()
}

fn retrieval_group(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    now: &RagChanges,
) -> SettingGroup {
    let hybrid = switch_row(
        settings,
        changes,
        spec("rag-hybrid", RagField::Hybrid, "Hybrid retrieval"),
        |settings| settings.hybrid,
        |settings, value| settings.hybrid = value,
    )
    .description("BM25 plus embeddings. Off skips vector search and needs no embedding model.");
    let embed_model = text_row(
        settings,
        changes,
        spec("rag-embed-model", RagField::EmbedModel, "Embedding model"),
        |settings| settings.embed_model.clone(),
        |settings, value| settings.embed_model = value,
    );
    let top_k = number_row(
        settings,
        changes,
        spec("rag-top-k", RagField::TopK, "Top-k chunks"),
        NumberFieldOptions {
            min: 1.0,
            max: 500.0,
            step: 1.0,
        },
        |settings| settings.top_k as f64,
        |settings, value| settings.top_k = value.round().clamp(1.0, 500.0) as usize,
    );
    let threshold = number_row(
        settings,
        changes,
        spec(
            "rag-threshold",
            RagField::SimilarityThreshold,
            "Similarity threshold",
        ),
        NumberFieldOptions {
            min: 0.0,
            max: 1.0,
            step: 0.01,
        },
        |settings| settings.similarity_threshold,
        |settings, value| settings.similarity_threshold = value.clamp(0.0, 1.0),
    )
    .description("Chunks below this score are dropped.");
    let corpus_dirs = text_row(
        settings,
        changes,
        spec("rag-corpus-dirs", RagField::CorpusDirs, "Folder corpora"),
        |settings| settings.corpus_dirs.join(", "),
        |settings, value| {
            settings.corpus_dirs = value
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(str::to_owned)
                .collect();
        },
    )
    .description("Comma-separated name=path pairs.")
    .vertical();

    mark_group(
        SettingGroup::new()
            .title(group_title("Retrieval", now.section(RagSection::Retrieval)))
            .item(hybrid.into_item())
            .item(embed_model.into_item())
            .item(top_k.into_item())
            .item(threshold.into_item())
            .item(corpus_dirs.into_item()),
        now,
        RagSection::Retrieval,
    )
}

fn parsing_group(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    now: &RagChanges,
) -> SettingGroup {
    let pdf_options: Vec<(SharedString, SharedString)> = PdfBackend::options()
        .iter()
        .map(|(backend, label)| {
            (
                SharedString::from(backend.label()),
                SharedString::from(*label),
            )
        })
        .collect();
    let pdf_backend = dropdown_row(
        settings,
        changes,
        spec("rag-pdf-backend", RagField::PdfBackend, "PDF backend"),
        move |_| pdf_options.clone(),
        |settings| settings.pdf_backend.label().to_owned(),
        |settings, value| {
            settings.pdf_backend = PdfBackend::options()
                .iter()
                .find(|(backend, _)| backend.label() == value)
                .map_or(PdfBackend::Extract, |(backend, _)| *backend);
        },
    )
    .description("Primary parser for PDF documents.");
    let sloppy_pdf = switch_row(
        settings,
        changes,
        spec("rag-sloppy-pdf", RagField::SloppyPdf, "Sloppy fallback"),
        |settings| settings.sloppy_pdf,
        |settings, value| settings.sloppy_pdf = value,
    )
    .description("Keep the panic-free PDF scavenger as a fallback.");
    let chunk_size = number_row(
        settings,
        changes,
        spec("rag-chunk-size", RagField::ChunkSize, "Chunk size"),
        NumberFieldOptions {
            min: 16.0,
            max: 32_768.0,
            step: 16.0,
        },
        |settings| settings.chunk_size as f64,
        |settings, value| settings.chunk_size = value.round().clamp(16.0, 32_768.0) as usize,
    );
    let chunk_overlap = number_row(
        settings,
        changes,
        spec("rag-chunk-overlap", RagField::ChunkOverlap, "Chunk overlap"),
        NumberFieldOptions {
            min: 0.0,
            max: 8_192.0,
            step: 8.0,
        },
        |settings| settings.chunk_overlap as f64,
        |settings, value| {
            settings.chunk_overlap = value.round().clamp(0.0, 8_192.0) as usize;
        },
    );

    mark_group(
        SettingGroup::new()
            .title(group_title(
                "Parsing and chunking",
                now.section(RagSection::Parsing),
            ))
            .item(pdf_backend.into_item())
            .item(sloppy_pdf.into_item())
            .item(chunk_size.into_item())
            .item(chunk_overlap.into_item())
            .footer(|_, cx| {
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Changing chunking only affects new ingestion, not stored chunks.")
            }),
        now,
        RagSection::Parsing,
    )
}

fn memory_group(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    now: &RagChanges,
) -> SettingGroup {
    let memory_options: Vec<(SharedString, SharedString)> = MemoryChoice::options()
        .iter()
        .map(|(choice, label)| {
            (
                SharedString::from(choice.label()),
                SharedString::from(*label),
            )
        })
        .collect();
    let memory = dropdown_row(
        settings,
        changes,
        spec("rag-memory", RagField::Memory, "Strategy"),
        move |_| memory_options.clone(),
        |settings| settings.memory.label().to_owned(),
        |settings, value| {
            settings.memory = MemoryChoice::options()
                .iter()
                .find(|(choice, _)| choice.label() == value)
                .map_or(MemoryChoice::Rewrite, |(choice, _)| *choice);
        },
    );
    let memory_model = text_row(
        settings,
        changes,
        spec("rag-memory-model", RagField::MemoryModel, "Memory model"),
        |settings| settings.memory_model.clone(),
        |settings, value| settings.memory_model = value,
    )
    .description("Small local model used for rewriting and summaries.");

    mark_group(
        SettingGroup::new()
            .title(group_title("Memory", now.section(RagSection::Memory)))
            .item(memory.into_item())
            .item(memory_model.into_item()),
        now,
        RagSection::Memory,
    )
}

/// The reference-ingestion group for a host's library page.
///
/// The GROBID server row belongs to the host's **External services** page;
/// hosts that show that page can use [`rag_reference_processing_group`] to
/// avoid a duplicate URL field.
pub fn rag_references_group(settings: &Entity<RagSettings>, changes: &Changes) -> SettingGroup {
    let grobid_url = text_row(
        settings,
        changes,
        spec("rag-grobid-url", RagField::GrobidUrl, "GROBID server"),
        |settings| settings.grobid_url.clone(),
        |settings, value| settings.grobid_url = value,
    )
    .description("Used when parsing a folder's PDFs into the bibliography.");
    let (rename_pdfs, openalex, resolve_citations) = reference_processing_rows(settings, changes);

    SettingGroup::new()
        .title("References")
        .item(grobid_url.into_item())
        .item(rename_pdfs.into_item())
        .item(openalex.into_item())
        .item(resolve_citations.into_item())
        .footer(|_, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    "Records merge into the Library bibliography (default .ragrig/references.bib).",
                )
        })
}

/// The reference-processing rows for a host that manages the GROBID server on
/// its own **External services** page.
pub fn rag_reference_processing_group(
    settings: &Entity<RagSettings>,
    changes: &Changes,
) -> SettingGroup {
    let (rename_pdfs, openalex, resolve_citations) = reference_processing_rows(settings, changes);

    SettingGroup::new()
        .title("References")
        .item(rename_pdfs.into_item())
        .item(openalex.into_item())
        .item(resolve_citations.into_item())
        .footer(|_, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(
                    "Records merge into the Library bibliography (default .ragrig/references.bib).",
                )
        })
}

fn reference_processing_rows(
    settings: &Entity<RagSettings>,
    changes: &Changes,
) -> (SettingRow, SettingRow, SettingRow) {
    let rename_pdfs = switch_row(
        settings,
        changes,
        spec("rag-rename-pdfs", RagField::RenamePdfs, "Rename PDFs"),
        |settings| settings.rename_pdfs,
        |settings, value| settings.rename_pdfs = value,
    )
    .description("Rename processed PDFs to Key - Authors - Title - Year.");
    let openalex = switch_row(
        settings,
        changes,
        spec("rag-openalex", RagField::OpenAlex, "OpenAlex completion"),
        |settings| settings.openalex,
        |settings, value| settings.openalex = value,
    )
    .description("Fill missing DOI and venue data from OpenAlex.");
    let resolve_citations = switch_row(
        settings,
        changes,
        spec(
            "rag-resolve-citations",
            RagField::ResolveCitations,
            "Use @key citations",
        ),
        |settings| settings.resolve_citations,
        |settings, value| settings.resolve_citations = value,
    )
    .description("Parse @key papers in a question and attach them as context.");

    (rename_pdfs, openalex, resolve_citations)
}

fn profiles_group(settings: &Entity<RagSettings>, cx: &App) -> SettingGroup {
    let profile_name = input_field(
        settings,
        |settings| settings.profile_name.clone(),
        |settings, value| settings.profile_name = value,
    );
    let profile_choice = dropdown_field(
        settings,
        profile_options(settings, cx),
        |settings| settings.profile_name.clone(),
        |settings, value| settings.profile_name = value,
    );
    let profile_actions = SettingItem::render({
        let settings = settings.clone();
        move |_, _, cx| profile_buttons(settings.clone(), cx)
    });

    SettingGroup::new()
        .title("Profiles")
        .item(SettingItem::new("Profile name", profile_name))
        .item(SettingItem::new("Load profile", profile_choice))
        .item(profile_actions)
        .footer(|_, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Profiles are JSON files in .ragrig/profiles; secrets are never written.")
        })
}

fn apply_group(settings: &Entity<RagSettings>, controller: &Entity<RagController>) -> SettingGroup {
    let settings = settings.clone();
    let controller = controller.clone();
    let apply =
        SettingItem::render(move |_, _, cx| apply_row(settings.clone(), controller.clone(), cx));
    SettingGroup::new().title("Apply").item(apply)
}

/// Build a complete [`Settings`] element for Ragrig configuration.
pub fn rag_settings(
    id: impl Into<ElementId>,
    settings: Entity<RagSettings>,
    controller: Entity<RagController>,
    cx: &App,
) -> Settings {
    Settings::new(id).page(rag_settings_page(settings, controller, cx))
}

fn profile_buttons(settings: Entity<RagSettings>, cx: &mut App) -> AnyElement {
    let save = {
        let settings = settings.clone();
        Button::new("rag-profile-save")
            .label("Save")
            .icon(gpui_kit::assets::IconName::Save)
            .small()
            .outline()
            .on_click(move |_, _, cx| {
                settings.update(cx, |settings, cx| {
                    if let Err(message) = settings.save_profile() {
                        settings.status = Some(message);
                    }
                    cx.notify();
                });
            })
    };
    let load = {
        let settings = settings.clone();
        Button::new("rag-profile-load")
            .label("Load")
            .icon(gpui_kit::assets::IconName::FolderOpen)
            .small()
            .outline()
            .on_click(move |_, _, cx| {
                let name = settings.read(cx).profile_name.trim().to_owned();
                if name.is_empty() {
                    settings.update(cx, |settings, cx| {
                        settings.status = Some("choose a profile first".to_owned());
                        cx.notify();
                    });
                    return;
                }
                settings.update(cx, |settings, cx| {
                    if let Err(message) = settings.load_profile(&name) {
                        settings.status = Some(message);
                    }
                    cx.notify();
                });
            })
    };
    let refresh = {
        let settings = settings.clone();
        Button::new("rag-profile-refresh")
            .label("Refresh")
            .icon(gpui_kit::assets::IconName::RefreshCw)
            .small()
            .ghost()
            .on_click(move |_, _, cx| {
                settings.update(cx, |settings, cx| {
                    settings.refresh_profiles();
                    cx.notify();
                });
            })
    };

    let status = settings.read(cx).status.clone();
    h_flex()
        .gap_2()
        .items_center()
        .child(save)
        .child(load)
        .child(refresh)
        .when_some(status, |row, status| {
            row.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(status),
            )
        })
        .into_any_element()
}

fn apply_row(
    settings: Entity<RagSettings>,
    controller: Entity<RagController>,
    cx: &mut App,
) -> AnyElement {
    let apply = {
        let settings = settings.clone();
        let controller = controller.clone();
        Button::new("rag-settings-apply")
            .label("Apply")
            .icon(gpui_kit::assets::IconName::Play)
            .primary()
            .small()
            .on_click(move |_, _, cx| match settings.read(cx).to_setup() {
                Ok(setup) => {
                    controller.update(cx, |controller, cx| controller.apply(setup, cx));
                    settings.update(cx, |settings, cx| {
                        settings.status = None;
                        cx.notify();
                    });
                }
                Err(message) => {
                    settings.update(cx, |settings, cx| {
                        settings.status = Some(message);
                        cx.notify();
                    });
                }
            })
    };

    let controller_status = controller.read(cx).status_line();
    let has_error = controller.read(cx).error().is_some();
    let status = settings
        .read(cx)
        .status
        .clone()
        .unwrap_or(controller_status);
    let color = if has_error {
        cx.theme().danger
    } else {
        cx.theme().muted_foreground
    };

    h_flex()
        .gap_3()
        .items_center()
        .child(apply)
        .child(div().text_sm().text_color(color).child(status))
        .into_any_element()
}

/// The page title, with a bullet while a section has unsaved changes.
pub(crate) fn group_title(label: &str, dirty: bool) -> SharedString {
    if dirty {
        format!("{label} ●").into()
    } else {
        SharedString::from(label)
    }
}

/// Append the "unsaved changes" footer to a group that differs from the agent.
fn mark_group(group: SettingGroup, now: &RagChanges, section: RagSection) -> SettingGroup {
    if !now.section(section) {
        return group;
    }
    group.footer(|_, cx| {
        div()
            .text_xs()
            .font_weight(FontWeight::BOLD)
            .text_color(dirty_color(cx))
            .child("● Unsaved changes — press Apply on Overview")
    })
}

/// The identity of one settings row: its keyed-state key, change field, and
/// visible title.
#[derive(Clone, Copy)]
struct FieldSpec {
    key: &'static str,
    field: RagField,
    title: &'static str,
}

fn spec(key: &'static str, field: RagField, title: &'static str) -> FieldSpec {
    FieldSpec { key, field, title }
}

fn dirty(changes: &Changes, field: RagField) -> impl Fn(&App) -> bool + 'static {
    let changes = changes.clone();
    move |cx| changes(cx).contains(field)
}

fn text_row<R, W>(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    spec: FieldSpec,
    read: R,
    write: W,
) -> SettingRow
where
    R: Fn(&RagSettings) -> String + 'static,
    W: Fn(&mut RagSettings, String) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    let control = text_control(
        spec.key,
        move |cx| read(reader.read(cx)),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                write(settings, value);
                cx.notify();
            });
        },
    );
    SettingRow::new(spec.title, control).dirty(dirty(changes, spec.field))
}

fn number_row<R, W>(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    spec: FieldSpec,
    options: NumberFieldOptions,
    read: R,
    write: W,
) -> SettingRow
where
    R: Fn(&RagSettings) -> f64 + 'static,
    W: Fn(&mut RagSettings, f64) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    let control = number_control(
        spec.key,
        options,
        move |cx| read(reader.read(cx)),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                write(settings, value);
                cx.notify();
            });
        },
    );
    SettingRow::new(spec.title, control).dirty(dirty(changes, spec.field))
}

fn switch_row<R, W>(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    spec: FieldSpec,
    read: R,
    write: W,
) -> SettingRow
where
    R: Fn(&RagSettings) -> bool + 'static,
    W: Fn(&mut RagSettings, bool) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    let control = switch_control(
        spec.key,
        move |cx| read(reader.read(cx)),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                write(settings, value);
                cx.notify();
            });
        },
    );
    SettingRow::new(spec.title, control).dirty(dirty(changes, spec.field))
}

fn dropdown_row<O, R, W>(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    spec: FieldSpec,
    options: O,
    read: R,
    write: W,
) -> SettingRow
where
    O: Fn(&App) -> Vec<(SharedString, SharedString)> + 'static,
    R: Fn(&RagSettings) -> String + 'static,
    W: Fn(&mut RagSettings, String) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    let control = dropdown_control(
        spec.key,
        options,
        move |cx| read(reader.read(cx)),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                write(settings, value);
                cx.notify();
            });
        },
    );
    SettingRow::new(spec.title, control).dirty(dirty(changes, spec.field))
}

fn token_row<R, W>(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    spec: FieldSpec,
    min_exponent: u32,
    max_exponent: u32,
    read: R,
    write: W,
) -> SettingRow
where
    R: Fn(&RagSettings) -> usize + 'static,
    W: Fn(&mut RagSettings, usize) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    let control = token_control(
        spec.key,
        min_exponent,
        max_exponent,
        move |cx| read(reader.read(cx)),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                write(settings, value);
                cx.notify();
            });
        },
    );
    SettingRow::new(spec.title, control).dirty(dirty(changes, spec.field))
}

fn profile_options(settings: &Entity<RagSettings>, cx: &App) -> Vec<(SharedString, SharedString)> {
    settings
        .read(cx)
        .profiles
        .iter()
        .filter(|name| !name.is_empty())
        .map(|name| {
            (
                SharedString::from(name.clone()),
                SharedString::from(name.clone()),
            )
        })
        .collect()
}

fn input_field<R, W>(
    settings: &Entity<RagSettings>,
    read: R,
    write: W,
) -> impl AnySettingField + use<R, W>
where
    R: Fn(&RagSettings) -> String + 'static,
    W: Fn(&mut RagSettings, String) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    SettingField::input(
        move |cx: &App| SharedString::from(read(reader.read(cx))),
        move |value: SharedString, cx: &mut App| {
            writer.update(cx, |settings, cx| {
                write(settings, value.to_string());
                cx.notify();
            });
        },
    )
}

fn dropdown_field<R, W>(
    settings: &Entity<RagSettings>,
    options: Vec<(SharedString, SharedString)>,
    read: R,
    write: W,
) -> impl AnySettingField + use<R, W>
where
    R: Fn(&RagSettings) -> String + 'static,
    W: Fn(&mut RagSettings, String) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    SettingField::dropdown(
        options,
        move |cx: &App| SharedString::from(read(reader.read(cx))),
        move |value: SharedString, cx: &mut App| {
            writer.update(cx, |settings, cx| {
                write(settings, value.to_string());
                cx.notify();
            });
        },
    )
}

fn provider_options() -> Vec<(SharedString, SharedString)> {
    ChatProvider::options()
        .iter()
        .map(|(provider, label)| {
            (
                SharedString::from(provider.label()),
                SharedString::from(*label),
            )
        })
        .collect()
}
