//! A titled side panel with a scrollable body.

use gpui_kit::{
    AnyElement, ElementId, FontWeight, InteractiveElement as _, IntoElement, ParentElement,
    RenderOnce, SharedString, Styled as _, Window,
    component::{ActiveTheme as _, h_flex, scroll::ScrollableElement as _, v_flex},
    div,
    prelude::FluentBuilder as _,
    px,
};

/// A reusable left/right dock surface.
///
/// The panel draws a header (title, optional subtitle, action buttons), a
/// vertically scrollable body, and an optional footer. It owns no state, so
/// hosts can rebuild it freely each frame.
#[derive(IntoElement)]
pub struct SidePanel {
    id: SharedString,
    title: SharedString,
    subtitle: Option<SharedString>,
    actions: Vec<AnyElement>,
    toolbar: Option<AnyElement>,
    children: Vec<AnyElement>,
    footer: Option<AnyElement>,
}

impl SidePanel {
    /// A panel with an element-ID prefix and a title.
    ///
    /// The ID must be unique in the window; it separates the scroll state of
    /// panels that otherwise look alike.
    pub fn new(id: impl Into<SharedString>, title: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            subtitle: None,
            actions: Vec::new(),
            toolbar: None,
            children: Vec::new(),
            footer: None,
        }
    }

    /// Add a muted second line under the title.
    pub fn subtitle(mut self, subtitle: impl Into<SharedString>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }

    /// Add an action to the header's trailing edge.
    pub fn action(mut self, action: impl IntoElement) -> Self {
        self.actions.push(action.into_any_element());
        self
    }

    /// Add a fixed toolbar between the header and the scrollable body.
    ///
    /// Unlike [`Self::child`], the toolbar does not scroll with the body, so
    /// pane-level controls stay reachable above a long list.
    pub fn toolbar(mut self, toolbar: impl IntoElement) -> Self {
        self.toolbar = Some(toolbar.into_any_element());
        self
    }

    /// Add a footer below the scrollable body.
    pub fn footer(mut self, footer: impl IntoElement) -> Self {
        self.footer = Some(footer.into_any_element());
        self
    }
}

impl ParentElement for SidePanel {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements);
    }
}

impl RenderOnce for SidePanel {
    fn render(self, _: &mut Window, cx: &mut gpui_kit::App) -> impl IntoElement {
        let theme = cx.theme();
        let body_id = ElementId::Name(format!("{}-body", self.id).into());

        let header = v_flex()
            .gap_1()
            .px_3()
            .py_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_lg()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(self.title),
                    )
                    .child(h_flex().gap_1().children(self.actions)),
            )
            .when_some(self.subtitle, |header, subtitle| {
                header.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(subtitle),
                )
            });

        let body = v_flex()
            .id(body_id)
            .flex_1()
            .min_h(px(0.))
            .gap_2()
            .px_3()
            .py_3()
            .overflow_y_scrollbar()
            .children(self.children);

        v_flex()
            .size_full()
            .bg(theme.background)
            .child(header)
            .when_some(self.toolbar, |panel, toolbar| {
                panel.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_b_1()
                        .border_color(theme.border)
                        .child(toolbar),
                )
            })
            .child(body)
            .when_some(self.footer, |panel, footer| {
                panel.child(
                    div()
                        .px_3()
                        .py_2()
                        .border_t_1()
                        .border_color(theme.border)
                        .child(footer),
                )
            })
    }
}
