//! The **External services** settings page.
//!
//! The page is split by source: an existing GROBID server gets URL discovery
//! and probing; the managed source gets the Docker image, lifecycle buttons,
//! and pull progress. It deliberately knows nothing about Ollama or other
//! services yet.

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

use crate::{
    SettingRow, control, dirty_header, dropdown_control,
    rag::{Changes, GrobidSource, RagField, RagSettings},
    switch_control, text_control,
};

use super::{GrobidContainerStatus, GrobidDockerStatus, GrobidProbeStatus, GrobidService};

/// Build the **External services** page.
///
/// Discovery, URL probing, and the managed container all report through the
/// [`GrobidService`] entity. The configured URL is written to `settings` when a
/// probe succeeds, so the host can persist it and the reference pipeline uses
/// it on the next extraction.
pub fn external_services_page(
    settings: Entity<RagSettings>,
    service: Entity<GrobidService>,
    changes: Changes,
    cx: &App,
) -> SettingPage {
    let now = changes(cx);
    let dirty = now.section(crate::rag::RagSection::Services);
    let source = settings.read(cx).grobid_source;
    let title: SharedString = if dirty {
        "External services ●".into()
    } else {
        "External services".into()
    };

    let mut page = SettingPage::new(title)
        .description(
            "GROBID turns PDF headers into bibliography records. Use a server that is already running or let the app pull and run the official Docker image.",
        )
        .default_open(true);
    if dirty {
        page = page.header_style(&dirty_header(cx));
    }

    page = page.group(source_group(&settings, &service, &changes));
    match source {
        GrobidSource::Existing => page.group(existing_group(&settings, &service, &changes)),
        GrobidSource::Managed => page.group(managed_group(&settings, &service, &changes)),
    }
}

fn source_group(
    settings: &Entity<RagSettings>,
    service: &Entity<GrobidService>,
    changes: &Changes,
) -> SettingGroup {
    let options: Vec<(SharedString, SharedString)> = GrobidSource::options()
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
        "grobid-source",
        move |_| options.clone(),
        move |cx| settings_for_read.read(cx).grobid_source.label().to_owned(),
        move |value, cx| {
            let source = GrobidSource::from_label(&value).unwrap_or(GrobidSource::Existing);
            settings_for_write.update(cx, |settings, cx| {
                settings.grobid_source = source;
                cx.notify();
            });
            if source == GrobidSource::Managed {
                service_for_write.update(cx, |service, cx| service.refresh(cx));
            }
        },
    );
    let dirty = {
        let changes = changes.clone();
        move |cx: &App| changes(cx).contains(RagField::GrobidSource)
    };

    SettingGroup::new().title("GROBID").item(
        SettingRow::new("Source", dropdown)
            .description("A running server can be discovered or entered by URL.")
            .dirty(dirty)
            .into_item(),
    )
}

fn existing_group(
    settings: &Entity<RagSettings>,
    service: &Entity<GrobidService>,
    changes: &Changes,
) -> SettingGroup {
    let url_row = url_row(settings, service, changes);

    let service_for_status = service.clone();
    let status = SettingItem::render(move |_, _, cx| {
        let service = service_for_status.read(cx);
        let color = if matches!(service.probe(), GrobidProbeStatus::Unreachable { .. }) {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let mut column = v_flex().gap_1().child(
            div()
                .text_sm()
                .text_color(color)
                .child(service.probe().summary()),
        );
        for probe in service.discovered() {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(match &probe.version {
                        Some(version) => format!("Found GROBID {version} at {}", probe.url),
                        None => format!("Found GROBID at {}", probe.url),
                    }),
            );
        }
        if let Some(notice) = service.notice() {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(notice.to_owned()),
            );
        }
        column.into_any_element()
    });

    SettingGroup::new()
        .title("Connection")
        .item(url_row.into_item())
        .item(status)
        .footer(|_, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("The probe checks /api/isalive; no GROBID request is sent while testing.")
        })
}

fn url_row(
    settings: &Entity<RagSettings>,
    service: &Entity<GrobidService>,
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
                clearer.update(cx, |service, cx| service.clear_probe(cx));
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
                        test_service.update(cx, |service, cx| service.test_url(url, cx));
                    }),
            )
            .child(
                Button::new("grobid-discover")
                    .label("Discover on localhost")
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        discover_service.update(cx, |service, cx| service.discover(cx));
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

fn managed_group(
    settings: &Entity<RagSettings>,
    service: &Entity<GrobidService>,
    changes: &Changes,
) -> SettingGroup {
    let settings_for_auto = settings.clone();
    let writer = settings.clone();
    let auto_start = switch_control(
        "grobid-auto-start",
        move |cx| settings_for_auto.read(cx).grobid_auto_start,
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.grobid_auto_start = value;
                cx.notify();
            });
        },
    );
    let changes_for_auto = changes.clone();
    let auto_start_row = SettingRow::new("Auto start", auto_start)
        .description("Start the managed GROBID container when the app opens.")
        .dirty(move |cx| changes_for_auto(cx).contains(RagField::GrobidAutoStart));

    let settings_for_image = settings.clone();
    let writer = settings.clone();
    let image = text_control(
        "grobid-image",
        move |cx| settings_for_image.read(cx).grobid_image.clone(),
        move |value, cx| {
            writer.update(cx, |settings, cx| {
                settings.grobid_image = value;
                cx.notify();
            });
        },
    );
    let changes_for_image = changes.clone();
    let image_row = SettingRow::new("Docker image", image)
        .description(
            "The full image adds the Deep Learning models (about 8 GB); a -crf tag is smaller and multi-architecture.",
        )
        .dirty(move |cx| changes_for_image(cx).contains(RagField::GrobidImage));

    let service_for_status = service.clone();
    let status = SettingItem::render(move |_, _, cx| {
        let service = service_for_status.read(cx);
        let docker_color = if matches!(service.docker(), GrobidDockerStatus::Unavailable { .. }) {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let container_color = if matches!(service.container(), GrobidContainerStatus::Failed { .. })
        {
            cx.theme().danger
        } else {
            cx.theme().muted_foreground
        };
        let mut column = v_flex()
            .gap_1()
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
                    .child(format!("Container: {}", service.container().summary())),
            );
        if let GrobidContainerStatus::Pulling {
            received, total, ..
        } = service.container()
        {
            column = column.child(progress(*received, *total, cx));
        }
        if let Some(notice) = service.notice() {
            column = column.child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(notice.to_owned()),
            );
        }
        column.into_any_element()
    });

    let service_for_actions = service.clone();
    let settings_for_actions = settings.clone();
    let actions = SettingItem::render(move |_, _, cx| {
        let service = service_for_actions.read(cx);
        let busy = service.is_busy();
        let running = matches!(service.container(), GrobidContainerStatus::Running);
        let missing = matches!(service.container(), GrobidContainerStatus::Missing);
        let stopped = matches!(service.container(), GrobidContainerStatus::Stopped);
        let image = settings_for_actions.read(cx).grobid_image.clone();

        let start_service = service_for_actions.clone();
        let stop_service = service_for_actions.clone();
        let remove_service = service_for_actions.clone();
        let refresh_service = service_for_actions.clone();

        let mut row = h_flex().w_full().items_center().gap_2();
        if running {
            row = row.child(
                Button::new("grobid-stop")
                    .label("Stop")
                    .small()
                    .outline()
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        stop_service.update(cx, |service, cx| service.stop(cx));
                    }),
            );
        } else {
            row = row.child(
                Button::new("grobid-start")
                    .label(if stopped { "Start" } else { "Pull and start" })
                    .small()
                    .primary()
                    .disabled(busy)
                    .on_click(move |_, _, cx| {
                        let image = image.clone();
                        start_service.update(cx, |service, cx| service.pull_and_start(image, cx));
                    }),
            );
        }
        let cancel_service = service_for_actions.clone();
        if service.is_cancelling() {
            row = row.child(
                Button::new("grobid-cancelling")
                    .label("Cancelling…")
                    .small()
                    .outline()
                    .disabled(true),
            );
        } else if service.is_cancellable() {
            row = row.child(
                Button::new("grobid-cancel")
                    .label("Cancel")
                    .small()
                    .outline()
                    .on_click(move |_, _, cx| {
                        cancel_service.update(cx, |service, cx| service.cancel(cx));
                    }),
            );
        }
        row = row.child(
            Button::new("grobid-remove")
                .label("Remove container")
                .small()
                .outline()
                .disabled(busy || missing)
                .on_click(move |_, _, cx| {
                    remove_service.update(cx, |service, cx| service.remove(cx));
                }),
        );
        row = row.child(
            Button::new("grobid-refresh")
                .label("Refresh")
                .small()
                .ghost()
                .on_click(move |_, _, cx| {
                    refresh_service.update(cx, |service, cx| service.refresh(cx));
                }),
        );
        row.into_any_element()
    });

    SettingGroup::new()
        .title("Managed container")
        .item(auto_start_row.into_item())
        .item(image_row.into_item())
        .item(actions)
        .item(status)
        .footer(|_, cx| {
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("The app stops the container when it quits. Removing the container leaves the image on disk.")
        })
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
