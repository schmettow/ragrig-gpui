//! A one-line task progress bar with a cancel button.

use std::rc::Rc;

use gpui_kit::{
    App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window,
    assets::IconName,
    component::{
        ActiveTheme as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        progress::Progress,
    },
    div,
    prelude::FluentBuilder as _,
    px,
};

type CancelHandler = Rc<dyn Fn(&mut Window, &mut App)>;

/// A labelled progress bar with an optional detail line and stop button.
///
/// The widget is presentation only: the host feeds it a fraction in `0.0..=1.0`
/// and a callback that stops the work.  Pair it with a worker progress snapshot
/// — for example [`CitationProgress`](crate::rag::CitationProgress) and
/// [`RagController::stop_citation_check`](crate::rag::RagController::stop_citation_check).
///
/// ```ignore
/// TaskProgress::new("citations", "Checking citations")
///     .fraction(progress.fraction())
///     .detail(progress.summary())
///     .on_cancel(move |_, cx| agent.update(cx, |agent, cx| agent.stop(citation_id, cx)))
/// ```
#[derive(IntoElement)]
pub struct TaskProgress {
    id: SharedString,
    label: SharedString,
    fraction: f32,
    detail: Option<SharedString>,
    on_cancel: Option<CancelHandler>,
}

impl TaskProgress {
    /// A progress row with a unique element-ID prefix.
    pub fn new(id: impl Into<SharedString>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            fraction: 0.0,
            detail: None,
            on_cancel: None,
        }
    }

    /// Set completion in `0.0..=1.0`; values outside the range are clamped.
    pub fn fraction(mut self, fraction: f32) -> Self {
        self.fraction = fraction.clamp(0.0, 1.0);
        self
    }

    /// Add a secondary line, e.g. `"6/12 · 1 failed"`.
    pub fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    /// Show a stop button that calls `handler` when clicked.
    pub fn on_cancel(mut self, handler: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.on_cancel = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for TaskProgress {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let Self {
            id,
            label,
            fraction,
            detail,
            on_cancel,
        } = self;
        let bar_id = format!("{id}-bar");
        let cancel_id = format!("{id}-cancel");
        let row_id = format!("{id}-task");

        h_flex()
            .debug_selector(move || row_id)
            .w_full()
            .items_center()
            .gap_2()
            .child(div().flex_none().text_sm().child(label))
            .child(
                div()
                    .flex_1()
                    .min_w(px(80.))
                    .debug_selector({
                        let bar_id = bar_id.clone();
                        move || bar_id.clone()
                    })
                    .child(
                        Progress::new(ElementId::Name(bar_id.into()))
                            .value(fraction * 100.0)
                            .small(),
                    ),
            )
            .when_some(detail, |row, detail| {
                row.child(
                    div()
                        .flex_none()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(detail),
                )
            })
            .when_some(on_cancel, |row, handler| {
                row.child(
                    Button::new(ElementId::Name(cancel_id.clone().into()))
                        .debug_selector(move || cancel_id)
                        .icon(IconName::Square)
                        .ghost()
                        .small()
                        .tooltip("Stop")
                        .on_click(move |_, window, cx| handler(window, cx)),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui_kit::{Context, Modifiers, Render, TestAppContext};

    use super::*;

    struct Host {
        clicks: Rc<Cell<usize>>,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let clicks = self.clicks.clone();
            div().w(px(640.)).h(px(120.)).child(
                TaskProgress::new("citations", "Checking citations")
                    .fraction(0.5)
                    .detail("6/12")
                    .on_cancel(move |_, _| clicks.set(clicks.get() + 1)),
            )
        }
    }

    #[gpui_kit::test]
    fn the_stop_button_reports_a_click(cx: &mut TestAppContext) {
        cx.update(crate::init);
        let clicks = Rc::new(Cell::new(0));
        let host_clicks = clicks.clone();
        let (_, cx) = cx.add_window_view(|_, _| Host {
            clicks: host_clicks,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("citations-bar").is_some(), "bar renders");
        let stop = cx
            .debug_bounds("citations-cancel")
            .expect("cancel button bounds");
        cx.simulate_click(stop.center(), Modifiers::default());
        cx.run_until_parked();
        assert_eq!(clicks.get(), 1);
    }

    #[gpui_kit::test]
    fn a_row_without_a_cancel_handler_has_no_button(cx: &mut TestAppContext) {
        struct Passive;
        impl Render for Passive {
            fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
                div()
                    .w(px(640.))
                    .h(px(120.))
                    .child(TaskProgress::new("passive", "Indexing").fraction(0.25))
            }
        }

        cx.update(crate::init);
        let (_, cx) = cx.add_window_view(|_, _| Passive);
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(cx.debug_bounds("passive-bar").is_some(), "bar renders");
        assert!(cx.debug_bounds("passive-cancel").is_none());
    }
}
