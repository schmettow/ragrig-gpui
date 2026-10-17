//! Settings helpers for chat configuration.

use gpui_kit::{
    App, Axis, ElementId, Entity, SharedString,
    component::setting::{
        NumberFieldOptions, SettingField, SettingGroup, SettingItem, SettingPage, Settings,
    },
};

use crate::model::ChatSettings;

/// Build the standard chat settings page.
///
/// The page reads and writes a [`ChatSettings`] entity, so a host can keep the
/// same values across renders and apply them only when the user confirms.
pub fn chat_settings_page(settings: Entity<ChatSettings>) -> SettingPage {
    let model = SettingField::dropdown(
        ChatSettings::MODELS
            .iter()
            .map(|(id, label)| (SharedString::from(*id), SharedString::from(*label)))
            .collect(),
        {
            let settings = settings.clone();
            move |cx: &App| SharedString::from(settings.read(cx).model.clone())
        },
        {
            let settings = settings.clone();
            move |value: SharedString, cx: &mut App| {
                settings.update(cx, |settings, cx| {
                    settings.model = value.to_string();
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
            let settings = settings.clone();
            move |cx: &App| settings.read(cx).temperature
        },
        {
            let settings = settings.clone();
            move |value: f64, cx: &mut App| {
                settings.update(cx, |settings, cx| {
                    settings.temperature = value.clamp(0.0, 2.0);
                    cx.notify();
                });
            }
        },
    );

    let max_tokens = SettingField::number_input(
        NumberFieldOptions {
            min: 1.0,
            max: 1_000_000.0,
            step: 1.0,
        },
        {
            let settings = settings.clone();
            move |cx: &App| settings.read(cx).max_tokens as f64
        },
        {
            let settings = settings.clone();
            move |value: f64, cx: &mut App| {
                settings.update(cx, |settings, cx| {
                    settings.max_tokens = value.round().clamp(1.0, 1_000_000.0) as usize;
                    cx.notify();
                });
            }
        },
    );

    let stream = SettingField::switch(
        {
            let settings = settings.clone();
            move |cx: &App| settings.read(cx).stream
        },
        {
            let settings = settings.clone();
            move |value: bool, cx: &mut App| {
                settings.update(cx, |settings, cx| {
                    settings.stream = value;
                    cx.notify();
                });
            }
        },
    );

    let system_prompt = SettingField::input(
        {
            let settings = settings.clone();
            move |cx: &App| SharedString::from(settings.read(cx).system_prompt.clone())
        },
        {
            let settings = settings.clone();
            move |value: SharedString, cx: &mut App| {
                settings.update(cx, |settings, cx| {
                    settings.system_prompt = value.to_string();
                    cx.notify();
                });
            }
        },
    );

    SettingPage::new("Chat")
        .description("Values are applied when the host confirms them.")
        .group(
            SettingGroup::new()
                .title("Model")
                .item(
                    SettingItem::new("Agent model", model)
                        .description("Model used for this conversation."),
                )
                .item(
                    SettingItem::new("Stream responses", stream)
                        .description("Show tokens while the answer is generated."),
                ),
        )
        .group(
            SettingGroup::new()
                .title("Generation")
                .item(
                    SettingItem::new("Temperature", temperature)
                        .description("Higher values make answers less predictable."),
                )
                .item(
                    SettingItem::new("Maximum tokens", max_tokens)
                        .description("Upper bound for one answer."),
                ),
        )
        .group(
            SettingGroup::new().title("Prompt").item(
                SettingItem::new("System prompt", system_prompt)
                    .description("Sent before every question.")
                    .layout(Axis::Vertical),
            ),
        )
}

/// Build a complete [`Settings`] element for chat configuration.
pub fn chat_settings(id: impl Into<ElementId>, settings: Entity<ChatSettings>) -> Settings {
    Settings::new(id).page(chat_settings_page(settings))
}
