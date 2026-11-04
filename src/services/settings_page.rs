//! The **External services** settings page.
//!
//! The page is split by service. Each service can point at a server that is
//! already running or let the app pull and run an official Docker image.
//! Docker problems (daemon stopped, missing socket permissions) are reported
//! once at page level and again inside a managed group.

use gpui_kit::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, SharedString, Styled as _,
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        setting::{SettingGroup, SettingItem, SettingPage},
        v_flex,
    },
    div, px, relative,
};

#[cfg(feature = "grobid")]
use crate::control;
use crate::{
    SettingRow, dirty_header, dropdown_control,
    rag::{Changes, RagField, RagSettings, ServiceSource},
    switch_control, text_control,
};

use super::{ContainerStatus, DockerStatus, ExternalServices, ServiceKind};

/// Build the **External services** page.
///
/// Discovery, URL probing, and the managed containers all report through the
/// [`ExternalServices`] entity. A successful probe or start writes the URL into
/// `settings`, so the host can persist it and the next request uses it.
pub fn external_services_page(
    settings: Entity<RagSettings>,
    service: Entity<ExternalServices>,
    changes: Changes,
    cx: &App,
) -> SettingPage {
    let dirty = {
        let changes = changes.clone();
        move |cx: &App| {
            let current = changes(cx);
            ServiceKind::ALL
                .iter()
                .any(|kind| group_dirty(&current, *kind))
        }
    };
    let is_dirty = dirty(cx);
    let title: SharedString = if is_dirty {
        "External services ●".into()
    } else {
        "External services".into()
    };

    let description: SharedString = if cfg!(feature = "grobid") {
        "GROBID turns PDF headers into bibliography records. Ollama supplies local chat and embedding models. llama.cpp serves a GGUF model through an OpenAI-compatible API. Use a server that is already running or let the app pull and run the official Docker image."
    } else {
        "Ollama supplies local chat and embedding models. llama.cpp serves a GGUF model through an OpenAI-compatible API. Use a server that is already running or let the app pull and run the official Docker image."
    }
    .into();
    let mut page = SettingPage::new(title)
        .description(description)
        .default_open(true);
    if is_dirty {
        page = page.header_style(&dirty_header(cx));
    }

    if has_managed_source(&settings, cx) {
        page = page.group(docker_group(settings.clone(), service.clone()));
    }
    #[cfg(feature = "grobid")]
    {
        page = page.group(grobid_group(
            settings.clone(),
            service.clone(),
            changes.clone(),
            cx,
        ));
    }
    page = page
        .group(ollama_group(
            settings.clone(),
            service.clone(),
            changes.clone(),
            cx,
        ))
        .group(llamacpp_group(settings, service, changes, cx));
    page
}

fn group_dirty(changes: &crate::rag::RagChanges, kind: ServiceKind) -> bool {
    let fields: &[RagField] = match kind {
        ServiceKind::Grobid => &[
            RagField::GrobidSource,
            RagField::GrobidUrl,
            RagField::GrobidImage,
            RagField::GrobidAutoStart,
        ],
        ServiceKind::Ollama => &[
            RagField::OllamaSource,
            RagField::OllamaImage,
            RagField::OllamaAutoStart,
        ],
        ServiceKind::LlamaCpp => &[
            RagField::LlamaCppSource,
            RagField::LlamaCppImage,
            RagField::LlamaCppModel,
            RagField::LlamaCppArgs,
            RagField::LlamaCppAutoStart,
        ],
    };
    fields.iter().any(|field| changes.contains(*field))
}

fn has_managed_source(settings: &Entity<RagSettings>, cx: &App) -> bool {
    let settings = settings.read(cx);
    ServiceKind::ALL
        .iter()
        .any(|kind| settings.service_source(*kind) == ServiceSource::Managed)
}

fn docker_group(settings: Entity<RagSettings>, service: Entity<ExternalServices>) -> SettingGroup {
    let status_service = service.clone();
    let status = SettingItem::render(move |_, _, cx| {
        let status = status_service.read(cx).docker();
        let color = if matches!(status, DockerStatus::Unavailable { .. }) {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        div()
            .text_sm()
            .text_color(color)
            .child(format!("Docker: {}", status.summary()))
            .into_any_element()
    });
    let refresh_service = service;
    let actions = SettingItem::render(move |_, _, cx| {
        let enabled = has_managed_source(&settings, cx);
        let refresh = refresh_service.clone();
        Button::new("services-docker-refresh")
            .label("Refresh status")
            .small()
            .outline()
            .disabled(!enabled)
            .on_click(move |_, _, cx| {
                refresh.update(cx, |service, cx| service.refresh_all(cx));
            })
            .into_any_element()
    });

    SettingGroup::new()
        .title("Docker")
        .description(
            "Docker Desktop or the Docker daemon must be running, and this user must be allowed to access the Docker socket.",
        )
        .item(actions)
        .item(status)
        .footer(|_, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Linux without socket access: add the user to the docker group and sign in again. Other systems: start Docker Desktop for this user and press Refresh.")
        })
}

#[cfg(feature = "grobid")]
fn grobid_group(
    settings: Entity<RagSettings>,
    service: Entity<ExternalServices>,
    changes: Changes,
    cx: &App,
) -> SettingGroup {
    let kind = ServiceKind::Grobid;
    let source = source_row(&settings, &service, &changes, kind);
    let mut group = SettingGroup::new()
        .title("GROBID")
        .description("Parses PDF headers and merges the result into the bibliography.")
        .item(source.into_item());

    if source_value(&settings, kind, cx) == ServiceSource::Existing {
        group = group
            .item(grobid_url_row(&settings, &service, &changes).into_item())
            .item(status_item(&settings, &service, kind));
    } else {
        group = group
            .item(auto_start_row(&settings, &changes, kind))
            .item(image_row(&settings, &changes, kind).into_item())
            .item(actions_item(&service, kind))
            .item(status_item(&settings, &service, kind));
    }
    group.footer(|_, cx| {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("The app stops the container when it quits. Removing the container leaves the image on disk.")
    })
}

fn ollama_group(
    settings: Entity<RagSettings>,
    service: Entity<ExternalServices>,
    changes: Changes,
    cx: &App,
) -> SettingGroup {
    let kind = ServiceKind::Ollama;
    let source = source_row(&settings, &service, &changes, kind);
    let mut group = SettingGroup::new()
        .title("Ollama")
        .description(
            "Serves local chat and embedding models at localhost:11434. The managed container keeps its models in the ragrig-ollama-models volume.",
        )
        .item(source.into_item());

    if source_value(&settings, kind, cx) == ServiceSource::Existing {
        group = group
            .item(probe_actions_item(&service, kind))
            .item(status_item(&settings, &service, kind));
    } else {
        group = group
            .item(auto_start_row(&settings, &changes, kind))
            .item(image_row(&settings, &changes, kind).into_item())
            .item(actions_item(&service, kind))
            .item(status_item(&settings, &service, kind));
    }
    group.footer(|_, cx| {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("Models are pulled on demand with Pull models; they survive container removal because the volume stays. If a server already answers on port 11434, the app uses it instead of starting a container. The app stops the container when it quits.")
    })
}

fn llamacpp_group(
    settings: Entity<RagSettings>,
    service: Entity<ExternalServices>,
    changes: Changes,
    cx: &App,
) -> SettingGroup {
    let kind = ServiceKind::LlamaCpp;
    let source = source_row(&settings, &service, &changes, kind);
    let mut group = SettingGroup::new()
        .title("llama.cpp")
        .description(
            "Serves one GGUF model through an OpenAI-compatible API. The chat URL lives in RAG → Chat.",
        )
        .item(source.into_item());

    if source_value(&settings, kind, cx) == ServiceSource::Existing {
        group = group
            .item(probe_actions_item(&service, kind))
            .item(status_item(&settings, &service, kind));
    } else {
        group = group
            .item(auto_start_row(&settings, &changes, kind))
            .item(image_row(&settings, &changes, kind).into_item())
            .item(llamacpp_model_row(&settings, &changes).into_item())
            .item(llamacpp_args_row(&settings, &changes).into_item())
            .item(actions_item(&service, kind))
            .item(status_item(&settings, &service, kind));
    }
    group.footer(|_, cx| {
        div()
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child("A Hugging Face repository is downloaded into the ragrig-llamacpp-models volume. An absolute .gguf path is mounted read-only from the host. If a server already answers on port 8080, the app uses it instead of starting a container. The app stops the container when it quits.")
    })
}

fn source_value(settings: &Entity<RagSettings>, kind: ServiceKind, cx: &App) -> ServiceSource {
    settings.read(cx).service_source(kind)
}

fn source_row(
    settings: &Entity<RagSettings>,
    service: &Entity<ExternalServices>,
    changes: &Changes,
    kind: ServiceKind,
) -> SettingRow {
    let options: Vec<(SharedString, SharedString)> = ServiceSource::options()
        .iter()
        .map(|(source, label)| {
            (
                SharedString::from(source.label()),
                SharedString::from(*label),
            )
        })
        .collect();
    let settings_for_read = settings.clone();
    let settings_for_write = settings.clone();
    let service_for_write = service.clone();
    let dropdown = dropdown_control(
        source_key(kind),
        move |_| options.clone(),
        move |cx| {
            settings_for_read
                .read(cx)
                .service_source(kind)
                .label()
                .to_owned()
        },
        move |value, cx| {
            let source = ServiceSource::from_label(&value).unwrap_or(ServiceSource::Existing);
            settings_for_write.update(cx, |settings, cx| {
                if settings.service_source(kind) != source {
                    settings.set_service_source(kind, source);
                    cx.notify();
                }
            });
            service_for_write.update(cx, |service, cx| {
                if source == ServiceSource::Managed {
                    service.refresh(kind, cx);
                } else {
                    service.probe_existing(kind, cx);
                }
            });
        },
    );
    let dirty = {
        let changes = changes.clone();
        move |cx: &App| changes(cx).contains(source_field(kind))
    };
    SettingRow::new("Source", dropdown)
        .description(match kind {
            ServiceKind::Grobid => "A running server can be discovered or entered by URL.",
            ServiceKind::Ollama => {
                "Use a daemon at localhost:11434 or let the app run the official container."
            }
            ServiceKind::LlamaCpp => {
                "Use an existing llama-server or let the app run the official container."
            }
        })
        .dirty(dirty)
}

fn auto_start_row(
    settings: &Entity<RagSettings>,
    changes: &Changes,
    kind: ServiceKind,
) -> SettingItem {
    let settings_for_read = settings.clone();
    let writer = settings.clone();
    let auto_start = switch_control(
        auto_start_key(kind),
        move |cx| settings_for_read.read(cx).service_auto_start(kind),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.set_service_auto_start(kind, value);
                cx.notify();
            });
        },
    );
    let changes = changes.clone();
    SettingRow::new("Auto start", auto_start)
        .description(format!(
            "Start the managed {} container when the app opens.",
            kind.label()
        ))
        .dirty(move |cx| changes(cx).contains(auto_start_field(kind)))
        .into_item()
}

fn image_row(settings: &Entity<RagSettings>, changes: &Changes, kind: ServiceKind) -> SettingRow {
    let settings_for_read = settings.clone();
    let writer = settings.clone();
    let image = text_control(
        image_key(kind),
        move |cx| settings_for_read.read(cx).service_image(kind).to_owned(),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.set_service_image(kind, value);
                cx.notify();
            });
        },
    );
    let changes = changes.clone();
    SettingRow::new("Docker image", image)
        .description(match kind {
            ServiceKind::Grobid => {
                "The full image adds the Deep Learning models (about 8 GB); a -crf tag is smaller and multi-architecture."
            }
            ServiceKind::Ollama => {
                "The official ollama/ollama image. The managed container runs on CPU; use an existing server for GPU acceleration. Model data lives in a volume."
            }
            ServiceKind::LlamaCpp => {
                "The official ghcr.io/ggml-org/llama.cpp:server image. The managed container runs on CPU; use an existing server for GPU acceleration."
            }
        })
        .dirty(move |cx| changes(cx).contains(image_field(kind)))
}

fn llamacpp_model_row(settings: &Entity<RagSettings>, changes: &Changes) -> SettingRow {
    let settings_for_read = settings.clone();
    let writer = settings.clone();
    let model = text_control(
        "ragrig-llamacpp-model",
        move |cx| settings_for_read.read(cx).llamacpp_model.clone(),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.llamacpp_model = value;
                cx.notify();
            });
        },
    );
    let changes = changes.clone();
    SettingRow::new("Model", model)
        .description(
            "Hugging Face repository (owner/repo[:quant]) or an absolute path to a .gguf file on this machine.",
        )
        .dirty(move |cx| changes(cx).contains(RagField::LlamaCppModel))
}

fn llamacpp_args_row(settings: &Entity<RagSettings>, changes: &Changes) -> SettingRow {
    let settings_for_read = settings.clone();
    let writer = settings.clone();
    let args = text_control(
        "ragrig-llamacpp-args",
        move |cx| settings_for_read.read(cx).llamacpp_args.clone(),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.llamacpp_args = value;
                cx.notify();
            });
        },
    );
    let changes = changes.clone();
    SettingRow::new("Extra arguments", args)
        .description("Space separated llama-server flags, for example `--n-gpu-layers all`. Do not change --host or --port.")
        .dirty(move |cx| changes(cx).contains(RagField::LlamaCppArgs))
}

#[cfg(feature = "grobid")]
fn grobid_url_row(
    settings: &Entity<RagSettings>,
    service: &Entity<ExternalServices>,
    changes: &Changes,
) -> SettingRow {
    let settings_for_read = settings.clone();
    let settings_for_write = settings.clone();
    let service_for_clear = service.clone();
    let settings_for_test = settings.clone();
    let service_for_test = service.clone();
    let service_for_discover = service.clone();

    let body = control(move |options, window, cx| {
        let settings = settings_for_read.clone();
        let writer = settings_for_write.clone();
        let clearer = service_for_clear.clone();
        let input = text_control(
            "grobid-url",
            move |cx| settings.read(cx).grobid_url.clone(),
            move |value, cx| {
                writer.update(cx, |settings, cx| {
                    settings.grobid_url = value;
                    cx.notify();
                });
                clearer.update(cx, |service, cx| {
                    service.clear_probe(ServiceKind::Grobid, cx);
                });
            },
        )(options, window, cx);

        let test_settings = settings_for_test.clone();
        let test_service = service_for_test.clone();
        let discover_service = service_for_discover.clone();
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .child(div().flex_1().child(input))
            .child(
                Button::new("grobid-test-url")
                    .label("Test")
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        let url = test_settings.read(cx).grobid_url.clone();
                        test_service.update(cx, |service, cx| {
                            service.test_url(ServiceKind::Grobid, url, cx);
                        });
                    }),
            )
            .child(
                Button::new("grobid-discover")
                    .label("Discover on localhost")
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        discover_service.update(cx, |service, cx| {
                            service.discover(ServiceKind::Grobid, cx);
                        });
                    }),
            )
            .into_any_element()
    });

    let changes = changes.clone();
    SettingRow::new("GROBID server", body)
        .description("A scheme is added when omitted. Discovery checks ports 8070 and 8080.")
        .dirty(move |cx| changes(cx).contains(RagField::GrobidUrl))
        .vertical()
}

fn probe_actions_item(service: &Entity<ExternalServices>, kind: ServiceKind) -> SettingItem {
    let service = service.clone();
    SettingItem::render(move |_, _, _cx| {
        let probe_service = service.clone();
        let discover_service = service.clone();
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .child(
                Button::new(SharedString::from(format!("{}-test", kind.container())))
                    .label("Test")
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        probe_service.update(cx, |service, cx| service.probe_existing(kind, cx));
                    }),
            )
            .child(
                Button::new(SharedString::from(format!("{}-discover", kind.container())))
                    .label("Discover on localhost")
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        discover_service.update(cx, |service, cx| service.discover(kind, cx));
                    }),
            )
            .into_any_element()
    })
}

fn actions_item(service: &Entity<ExternalServices>, kind: ServiceKind) -> SettingItem {
    let service_for_actions = service.clone();
    SettingItem::render(move |_, _, cx| {
        let service = service_for_actions.read(cx);
        let busy = service.is_busy(kind);
        let running = matches!(
            service.container(kind),
            ContainerStatus::Running | ContainerStatus::Provisioning { .. }
        );
        let missing = matches!(service.container(kind), ContainerStatus::Missing);
        let stopped = matches!(service.container(kind), ContainerStatus::Stopped);

        let start_service = service_for_actions.clone();
        let stop_service = service_for_actions.clone();
        let remove_service = service_for_actions.clone();
        let refresh_service = service_for_actions.clone();
        let cancel_service = service_for_actions.clone();
        let models_service = service_for_actions.clone();
        let models = service_for_actions.read(cx).pullable_models(cx);

        let mut row = h_flex().w_full().items_center().gap_2();
        if running {
            row = row.child(
                Button::new(SharedString::from(format!("{}-stop", kind.container())))
                    .label("Stop")
                    .small()
                    .outline()
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        stop_service.update(cx, |service, cx| service.stop(kind, cx));
                    }),
            );
            if kind == ServiceKind::Ollama {
                row = row.child(
                    Button::new("ollama-pull-models")
                        .label("Pull models")
                        .small()
                        .outline()
                        .disabled(busy || models.is_empty())
                        .on_click(move |_, _, cx| {
                            models_service.update(cx, |service, cx| service.pull_models(cx));
                        }),
                );
            }
        } else {
            row = row.child(
                Button::new(SharedString::from(format!("{}-start", kind.container())))
                    .label(if stopped { "Start" } else { "Pull and start" })
                    .small()
                    .primary()
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        start_service.update(cx, |service, cx| service.start(kind, cx));
                    }),
            );
        }
        if service.is_cancelling(kind) {
            row = row.child(
                Button::new(SharedString::from(format!(
                    "{}-cancelling",
                    kind.container()
                )))
                .label("Cancelling…")
                .small()
                .outline()
                .disabled(true),
            );
        } else if service.is_cancellable(kind) {
            row = row.child(
                Button::new(SharedString::from(format!("{}-cancel", kind.container())))
                    .label("Cancel")
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        cancel_service.update(cx, |service, cx| service.cancel(kind, cx));
                    }),
            );
        }
        row = row.child(
            Button::new(SharedString::from(format!("{}-remove", kind.container())))
                .label("Remove container")
                .small()
                .outline()
                .disabled(busy || missing)
                .on_click(move |_, _, cx| {
                    remove_service.update(cx, |service, cx| service.remove(kind, cx));
                }),
        );
        row = row.child(
            Button::new(SharedString::from(format!("{}-refresh", kind.container())))
                .label("Refresh")
                .small()
                .ghost()
                .on_click(move |_, _, cx| {
                    refresh_service.update(cx, |service, cx| service.refresh(kind, cx));
                }),
        );
        row.into_any_element()
    })
}

fn status_item(
    settings: &Entity<RagSettings>,
    service: &Entity<ExternalServices>,
    kind: ServiceKind,
) -> SettingItem {
    let service_for_status = service.clone();
    let settings_for_status = settings.clone();
    SettingItem::render(move |_, _, cx| {
        let service = service_for_status.read(cx);
        let source = settings_for_status.read(cx).service_source(kind);
        let docker_color = if matches!(service.docker(), DockerStatus::Unavailable { .. }) {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let container_color = if matches!(service.container(kind), ContainerStatus::Failed { .. }) {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let mut column = v_flex().gap_1();

        if source == ServiceSource::Managed {
            column = column
                .child(
                    div()
                        .text_sm()
                        .text_color(docker_color)
                        .child(format!("Docker: {}", service.docker().summary())),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(container_color)
                        .child(format!("Container: {}", service.container(kind).summary())),
                );
        } else {
            column = column.child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(service.probe(kind).summary()),
            );
        }

        match service.container(kind) {
            ContainerStatus::Pulling {
                received, total, ..
            }
            | ContainerStatus::Provisioning {
                received, total, ..
            } => {
                column = column.child(progress(*received, *total, cx));
            }
            _ => {}
        }
        for probe in service.discovered(kind) {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(match &probe.version {
                        Some(version) => {
                            format!("Found {} {version} at {}", kind.label(), probe.url)
                        }
                        None => format!("Found {} at {}", kind.label(), probe.url),
                    }),
            );
        }
        if kind == ServiceKind::Ollama && source == ServiceSource::Managed {
            let models = service.pullable_models(cx);
            if !models.is_empty() {
                column = column.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(format!("Models to pull: {}", models.join(", "))),
                );
            }
        }
        if let Some(notice) = service.notice(kind) {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(notice.to_owned()),
            );
        }
        column.into_any_element()
    })
}

fn source_key(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Grobid => "grobid-source",
        ServiceKind::Ollama => "ollama-source",
        ServiceKind::LlamaCpp => "llamacpp-source",
    }
}

fn auto_start_key(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Grobid => "grobid-auto-start",
        ServiceKind::Ollama => "ollama-auto-start",
        ServiceKind::LlamaCpp => "llamacpp-auto-start",
    }
}

fn image_key(kind: ServiceKind) -> &'static str {
    match kind {
        ServiceKind::Grobid => "grobid-image",
        ServiceKind::Ollama => "ollama-image",
        ServiceKind::LlamaCpp => "llamacpp-image",
    }
}

fn source_field(kind: ServiceKind) -> RagField {
    match kind {
        ServiceKind::Grobid => RagField::GrobidSource,
        ServiceKind::Ollama => RagField::OllamaSource,
        ServiceKind::LlamaCpp => RagField::LlamaCppSource,
    }
}

fn image_field(kind: ServiceKind) -> RagField {
    match kind {
        ServiceKind::Grobid => RagField::GrobidImage,
        ServiceKind::Ollama => RagField::OllamaImage,
        ServiceKind::LlamaCpp => RagField::LlamaCppImage,
    }
}

fn auto_start_field(kind: ServiceKind) -> RagField {
    match kind {
        ServiceKind::Grobid => RagField::GrobidAutoStart,
        ServiceKind::Ollama => RagField::OllamaAutoStart,
        ServiceKind::LlamaCpp => RagField::LlamaCppAutoStart,
    }
}

fn progress(received: u64, total: u64, cx: &App) -> AnyElement {
    let ratio = if total > 0 {
        (received as f32 / total as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    div()
        .h(px(5.0))
        .w_full()
        .rounded_full()
        .bg(cx.theme().muted)
        .child(
            div()
                .h_full()
                .w(relative(ratio))
                .rounded_full()
                .bg(cx.theme().primary),
        )
        .into_any_element()
}
