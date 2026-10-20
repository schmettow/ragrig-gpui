//! The agent component status bar.

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    Anchor, Animation, AnimationExt as _, AnyElement, App, Div, ElementId, FontWeight, Hsla,
    IntoElement, ParentElement as _, RenderOnce, SharedString, Styled as _, Window,
    assets::IconName,
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        popover::Popover,
        switch::Switch,
        v_flex,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

use crate::model::{AgentComponent, ComponentStatus};

/// What the user asked the agent bar to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentBarEvent {
    /// Open the configuration surface for a component id.
    Configure(String),
    /// Flip a switch in a component popover.
    Toggle {
        /// Component id that owns the switch.
        component: String,
        /// Switch id.
        toggle: String,
        /// The state the user asked for.
        enabled: bool,
    },
    /// Open the general agent settings.
    OpenSettings,
}

type AgentHandler = Rc<dyn Fn(AgentBarEvent, &mut Window, &mut App)>;

/// A row of pipeline components with status dots and detail popovers.
///
/// Each component opens a popover anchored to its icon. The popover shows the
/// label, status, and detail line, and emits [`AgentBarEvent::Configure`] when
/// the host should open its configuration.
#[derive(IntoElement)]
pub struct AgentBar {
    id: SharedString,
    components: Vec<AgentComponent>,
    on_event: Option<AgentHandler>,
}

impl AgentBar {
    /// A bar with a unique element-ID prefix.
    pub fn new(
        id: impl Into<SharedString>,
        components: impl IntoIterator<Item = AgentComponent>,
    ) -> Self {
        Self {
            id: id.into(),
            components: components.into_iter().collect(),
            on_event: None,
        }
    }

    /// Receive configuration and settings requests.
    pub fn on_event(
        mut self,
        handler: impl Fn(AgentBarEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for AgentBar {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let handler = self.on_event;
        let prefix = self.id;

        let components = self
            .components
            .into_iter()
            .enumerate()
            .map(|(index, component)| {
                component_button(&prefix, index, component, handler.clone(), cx)
            })
            .collect::<Vec<_>>();

        let settings_handler = handler;
        let settings_id = ElementId::Name(format!("{prefix}-settings").into());
        h_flex()
            .items_center()
            .gap_1()
            .px_2()
            .py_1()
            .children(components)
            .child(div().flex_1())
            .child(
                Button::new(settings_id)
                    .icon(IconName::Settings)
                    .ghost()
                    .small()
                    .tooltip("Agent settings")
                    .on_click(move |_, window, cx| {
                        if let Some(handler) = &settings_handler {
                            handler(AgentBarEvent::OpenSettings, window, cx);
                        }
                    }),
            )
    }
}

fn component_button(
    prefix: &SharedString,
    index: usize,
    component: AgentComponent,
    handler: Option<AgentHandler>,
    cx: &mut App,
) -> Div {
    let theme = cx.theme();
    let color = status_color(component.status, cx);
    let popover_id = ElementId::Name(format!("{prefix}-{index}-popover").into());
    let trigger_id = ElementId::Name(format!("{prefix}-{index}-trigger").into());
    let pulse_id = ElementId::Name(format!("{prefix}-{index}-pulse").into());

    let tooltip = if let Some(detail) = &component.detail {
        format!(
            "{} · {}\n{}",
            component.label,
            component.status.label(),
            detail
        )
    } else {
        format!("{} · {}", component.label, component.status.label())
    };

    let content_component = component.clone();
    let content_handler = handler.clone();
    let content_prefix = prefix.clone();
    let popover = Popover::new(popover_id)
        .anchor(Anchor::TopLeft)
        .trigger(
            Button::new(trigger_id)
                .icon(component.kind.icon())
                .ghost()
                .small()
                .rounded(px(4.))
                .bg(theme.muted.opacity(0.18))
                .tooltip(tooltip),
        )
        .content(move |_, _, cx| {
            let popover_entity = cx.entity();
            let configure_handler = content_handler.clone();
            let configure_id =
                ElementId::Name(format!("{content_prefix}-{index}-configure").into());
            let component = content_component.clone();
            let id = component.id.clone();
            let configure_popover = popover_entity.clone();

            let toggle_rows = component
                .toggles
                .iter()
                .map(|toggle| {
                    let handler = content_handler.clone();
                    let component_id = component.id.clone();
                    let toggle_id = toggle.id.clone();
                    h_flex()
                        .justify_between()
                        .items_center()
                        .gap_3()
                        .child(
                            v_flex()
                                .flex_1()
                                .child(div().text_sm().child(toggle.label.clone()))
                                .when_some(toggle.detail.clone(), |column, detail| {
                                    column.child(
                                        div()
                                            .text_xs()
                                            .text_color(cx.theme().muted_foreground)
                                            .child(detail),
                                    )
                                }),
                        )
                        .child(
                            Switch::new(ElementId::Name(
                                format!("{content_prefix}-{index}-toggle-{toggle_id}").into(),
                            ))
                            .checked(toggle.enabled)
                            .on_click(move |enabled, window, cx| {
                                if let Some(handler) = &handler {
                                    handler(
                                        AgentBarEvent::Toggle {
                                            component: component_id.clone(),
                                            toggle: toggle_id.clone(),
                                            enabled: *enabled,
                                        },
                                        window,
                                        cx,
                                    );
                                }
                            }),
                        )
                        .into_any_element()
                })
                .collect::<Vec<_>>();

            v_flex()
                .w(px(280.))
                .p_3()
                .gap_2()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().child(component.kind.icon()))
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(component.label.clone()),
                        ),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(status_color(component.status, cx))
                        .child(component.status.label()),
                )
                .when_some(component.detail.clone(), |card, detail| {
                    card.child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(detail),
                    )
                })
                .when(!toggle_rows.is_empty(), |card| {
                    card.child(div().h(px(1.)).bg(cx.theme().border))
                        .child(v_flex().gap_2().children(toggle_rows))
                })
                .child(
                    Button::new(configure_id)
                        .label("Configure")
                        .outline()
                        .small()
                        .on_click(move |_, window, cx| {
                            if let Some(handler) = &configure_handler {
                                handler(AgentBarEvent::Configure(id.clone()), window, cx);
                            }
                            configure_popover.update(cx, |state, cx| state.dismiss(window, cx));
                        }),
                )
        });

    let dot = div()
        .absolute()
        .bottom(px(1.))
        .right(px(1.))
        .size(px(7.))
        .rounded_full()
        .border_1()
        .border_color(theme.background)
        .bg(color);

    let dot: AnyElement = if component.status.is_busy() {
        dot.with_animation(
            pulse_id,
            Animation::new(Duration::from_millis(900)).repeat(),
            |dot, delta| dot.opacity(0.35 + 0.65 * (1.0 - delta)),
        )
        .into_any_element()
    } else {
        dot.into_any_element()
    };

    div().relative().child(popover).child(dot)
}

fn status_color(status: ComponentStatus, cx: &App) -> Hsla {
    let theme = cx.theme();
    match status {
        ComponentStatus::Inactive => theme.muted_foreground,
        ComponentStatus::Ready => theme.success,
        ComponentStatus::Busy => theme.info,
        ComponentStatus::Error => theme.danger,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ComponentKind;

    #[test]
    fn a_bar_collects_components() {
        let bar = AgentBar::new(
            "agent",
            [
                AgentComponent::new("chat", ComponentKind::Chat, "DeepSeek"),
                AgentComponent::new("embed", ComponentKind::Embedder, "nomic"),
            ],
        );
        assert_eq!(bar.components.len(), 2);
    }
}
