//! The full Ragrig settings page.

use gpui_kit::{
    AnyElement, App, Axis, ElementId, Entity, IntoElement, ParentElement as _, SharedString,
    Styled as _,
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

use super::{ChatProvider, MemoryChoice, PdfBackend, RagController, RagSettings};

/// Build the full Ragrig settings page.
///
/// The Apply row validates [`RagSettings`] and asks the [`RagController`] to
/// rebuild the agent. Profile actions are filesystem-only; they do not touch
/// the running agent until Apply is pressed.
pub fn rag_settings_page(
    settings: Entity<RagSettings>,
    controller: Entity<RagController>,
    cx: &App,
) -> SettingPage {
    let workspace = input_field(
        &settings,
        |settings| settings.workspace.clone(),
        |settings, value| settings.workspace = value,
    );
    let bibliography = input_field(
        &settings,
        |settings| settings.bibliography.clone(),
        |settings, value| settings.bibliography = value,
    );

    let provider = dropdown_field(
        &settings,
        provider_options(),
        |settings| settings.provider.label().to_owned(),
        |settings, value| {
            settings.provider = ChatProvider::options()
                .iter()
                .find(|(_, label)| *label == value)
                .map_or(ChatProvider::Ollama, |(provider, _)| *provider);
        },
    );
    let ollama_model = input_field(
        &settings,
        |settings| settings.ollama_model.clone(),
        |settings, value| settings.ollama_model = value,
    );
    let deepseek_model = input_field(
        &settings,
        |settings| settings.deepseek_model.clone(),
        |settings, value| settings.deepseek_model = value,
    );
    let temperature = number_field(
        &settings,
        NumberFieldOptions {
            min: 0.0,
            max: 2.0,
            step: 0.1,
        },
        |settings| settings.temperature,
        |settings, value| settings.temperature = value.clamp(0.0, 2.0),
    );
    let max_tokens = number_field(
        &settings,
        NumberFieldOptions {
            min: 1.0,
            max: 1_000_000.0,
            step: 1.0,
        },
        |settings| settings.max_tokens as f64,
        |settings, value| settings.max_tokens = value.round().clamp(1.0, 1_000_000.0) as usize,
    );
    let context_tokens = number_field(
        &settings,
        NumberFieldOptions {
            min: 256.0,
            max: 1_000_000.0,
            step: 256.0,
        },
        |settings| settings.context_tokens as f64,
        |settings, value| {
            settings.context_tokens = value.round().clamp(256.0, 1_000_000.0) as usize;
        },
    );
    let stream = switch_field(
        &settings,
        |settings| settings.stream,
        |settings, value| settings.stream = value,
    );
    let system_prompt = input_field(
        &settings,
        |settings| settings.system_prompt.clone(),
        |settings, value| settings.system_prompt = value,
    );

    let embed_model = input_field(
        &settings,
        |settings| settings.embed_model.clone(),
        |settings, value| settings.embed_model = value,
    );
    let top_k = number_field(
        &settings,
        NumberFieldOptions {
            min: 1.0,
            max: 500.0,
            step: 1.0,
        },
        |settings| settings.top_k as f64,
        |settings, value| settings.top_k = value.round().clamp(1.0, 500.0) as usize,
    );
    let threshold = number_field(
        &settings,
        NumberFieldOptions {
            min: 0.0,
            max: 1.0,
            step: 0.01,
        },
        |settings| settings.similarity_threshold,
        |settings, value| settings.similarity_threshold = value.clamp(0.0, 1.0),
    );

    let pdf_backend = dropdown_field(
        &settings,
        PdfBackend::options()
            .iter()
            .map(|(backend, label)| {
                (
                    SharedString::from(backend.label()),
                    SharedString::from(*label),
                )
            })
            .collect(),
        |settings| settings.pdf_backend.label().to_owned(),
        |settings, value| {
            settings.pdf_backend = PdfBackend::options()
                .iter()
                .find(|(backend, _)| backend.label() == value)
                .map_or(PdfBackend::Extract, |(backend, _)| *backend);
        },
    );
    let sloppy_pdf = switch_field(
        &settings,
        |settings| settings.sloppy_pdf,
        |settings, value| settings.sloppy_pdf = value,
    );
    let chunk_size = number_field(
        &settings,
        NumberFieldOptions {
            min: 16.0,
            max: 32_768.0,
            step: 16.0,
        },
        |settings| settings.chunk_size as f64,
        |settings, value| settings.chunk_size = value.round().clamp(16.0, 32_768.0) as usize,
    );
    let chunk_overlap = number_field(
        &settings,
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

    let memory = dropdown_field(
        &settings,
        MemoryChoice::options()
            .iter()
            .map(|(choice, label)| {
                (
                    SharedString::from(choice.label()),
                    SharedString::from(*label),
                )
            })
            .collect(),
        |settings| settings.memory.label().to_owned(),
        |settings, value| {
            settings.memory = MemoryChoice::options()
                .iter()
                .find(|(choice, _)| choice.label() == value)
                .map_or(MemoryChoice::Rewrite, |(choice, _)| *choice);
        },
    );
    let memory_model = input_field(
        &settings,
        |settings| settings.memory_model.clone(),
        |settings, value| settings.memory_model = value,
    );

    #[cfg(feature = "grobid")]
    let grobid_url = input_field(
        &settings,
        |settings| settings.grobid_url.clone(),
        |settings, value| settings.grobid_url = value,
    );
    #[cfg(feature = "grobid")]
    let rename_pdfs = switch_field(
        &settings,
        |settings| settings.rename_pdfs,
        |settings, value| settings.rename_pdfs = value,
    );
    #[cfg(feature = "grobid")]
    let openalex = switch_field(
        &settings,
        |settings| settings.openalex,
        |settings, value| settings.openalex = value,
    );
    #[cfg(feature = "grobid")]
    let resolve_citations = switch_field(
        &settings,
        |settings| settings.resolve_citations,
        |settings, value| settings.resolve_citations = value,
    );

    let profile_name = input_field(
        &settings,
        |settings| settings.profile_name.clone(),
        |settings, value| settings.profile_name = value,
    );
    let profile_choice = dropdown_field(
        &settings,
        profile_options(&settings, cx),
        |settings| settings.profile_name.clone(),
        |settings, value| settings.profile_name = value,
    );

    let profile_actions = SettingItem::render({
        let settings = settings.clone();
        move |_, _, cx| profile_buttons(settings.clone(), cx)
    });

    let apply = SettingItem::render({
        let settings = settings.clone();
        let controller = controller.clone();
        move |_, _, cx| apply_row(settings.clone(), controller.clone(), cx)
    });

    let mut page = SettingPage::new("RAG")
        .description(
            "Ragrig workspace, models, retrieval, and memory. Press Apply to rebuild the agent.",
        )
        .default_open(true)
        .group(
            SettingGroup::new()
                .title("Workspace")
                .item(SettingItem::new("Workspace", workspace).description(
                    "Store, sessions, and profiles live under this folder (.ragrig).",
                ))
                .item(
                    SettingItem::new("Bibliography", bibliography).description(
                        "The .bib the library reads and GROBID writes. Empty uses .ragrig/references.bib.",
                    ),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Chat")
                .item(
                    SettingItem::new("Provider", provider)
                        .description("DeepSeek reads DEEPSEEK_API_KEY from the environment."),
                )
                .item(
                    SettingItem::new("Ollama model", ollama_model)
                        .description("Used when the provider is Ollama."),
                )
                .item(
                    SettingItem::new("DeepSeek model", deepseek_model)
                        .description("Used when the provider is DeepSeek."),
                )
                .item(SettingItem::new("Stream responses", stream))
                .item(
                    SettingItem::new("Temperature", temperature)
                        .description("0 is deterministic; 2 is very loose."),
                )
                .item(SettingItem::new("Maximum tokens", max_tokens))
                .item(SettingItem::new("Context tokens", context_tokens))
                .item(
                    SettingItem::new("System prompt", system_prompt)
                        .description("Sent before every question. Applied on the next Apply.")
                        .layout(Axis::Vertical),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Retrieval")
                .item(SettingItem::new("Embedding model", embed_model))
                .item(SettingItem::new("Top-k chunks", top_k))
                .item(
                    SettingItem::new("Similarity threshold", threshold)
                        .description("Chunks below this score are dropped."),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Parsing and chunking")
                .item(
                    SettingItem::new("PDF backend", pdf_backend)
                        .description("Primary parser for PDF documents."),
                )
                .item(
                    SettingItem::new("Sloppy fallback", sloppy_pdf)
                        .description("Keep the panic-free PDF scavenger as a fallback."),
                )
                .item(SettingItem::new("Chunk size", chunk_size))
                .item(SettingItem::new("Chunk overlap", chunk_overlap))
                .footer(|_, cx| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Changing chunking only affects new ingestion, not stored chunks.")
                }),
        )
        .group(
            SettingGroup::new()
                .title("Memory")
                .item(SettingItem::new("Strategy", memory))
                .item(
                    SettingItem::new("Memory model", memory_model)
                        .description("Small local model used for rewriting and summaries."),
                ),
        );

    #[cfg(feature = "grobid")]
    {
        page = page.group(
            SettingGroup::new()
                .title("References")
                .item(
                    SettingItem::new("GROBID server", grobid_url)
                        .description("Used when parsing a folder's PDFs into the bibliography."),
                )
                .item(
                    SettingItem::new("Rename PDFs", rename_pdfs)
                        .description("Rename processed PDFs to Key - Authors - Title - Year."),
                )
                .item(
                    SettingItem::new("OpenAlex completion", openalex)
                        .description("Fill missing DOI and venue data from OpenAlex."),
                )
                .item(
                    SettingItem::new("Use @key citations", resolve_citations)
                        .description("Parse @key papers in a question and attach them as context."),
                )
                .footer(|_, cx| {
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("Records merge into the bibliography above (default .ragrig/references.bib).")
                }),
        );
    }

    page = page
        .group(
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
                }),
        )
        .group(SettingGroup::new().title("Apply").item(apply));

    page
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

fn number_field<R, W>(
    settings: &Entity<RagSettings>,
    options: NumberFieldOptions,
    read: R,
    write: W,
) -> impl AnySettingField + use<R, W>
where
    R: Fn(&RagSettings) -> f64 + 'static,
    W: Fn(&mut RagSettings, f64) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    SettingField::number_input(
        options,
        move |cx: &App| read(reader.read(cx)),
        move |value: f64, cx: &mut App| {
            writer.update(cx, |settings, cx| {
                write(settings, value);
                cx.notify();
            });
        },
    )
}

fn switch_field<R, W>(
    settings: &Entity<RagSettings>,
    read: R,
    write: W,
) -> impl AnySettingField + use<R, W>
where
    R: Fn(&RagSettings) -> bool + 'static,
    W: Fn(&mut RagSettings, bool) + 'static,
{
    let reader = settings.clone();
    let writer = settings.clone();
    SettingField::switch(
        move |cx: &App| read(reader.read(cx)),
        move |value: bool, cx: &mut App| {
            writer.update(cx, |settings, cx| {
                write(settings, value);
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
