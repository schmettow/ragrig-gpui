//! The chat composer: an auto-growing input with send/stop controls.

use gpui_kit::{
    App, AppContext as _, Context, ElementId, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Subscription, Window,
    assets::IconName,
    component::{
        ActiveTheme as _, Disableable as _, Icon, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
        input::{InputEvent, Textarea, TextareaState},
        v_flex,
    },
    div,
};

use crate::icon::ChatIcon;

/// What the host should do after the composer was used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComposerEvent {
    /// The user submitted the text; it has been trimmed.
    Submit(String),
    /// The user asked to stop the current generation.
    Stop,
}

/// A reusable chat input.
///
/// The composer owns its [`TextareaState`] and emits [`ComposerEvent`] instead
/// of calling the host directly. A host creates one entity, subscribes to it,
/// and calls [`set_streaming`](Self::set_streaming) when the worker starts and
/// stops.
pub struct Composer {
    textarea: Entity<TextareaState>,
    streaming: bool,
    disabled: bool,
    pending_clear: bool,
    hint: Option<SharedString>,
    send_icon: ChatIcon,
    _input: Subscription,
}

impl Composer {
    /// Create a composer with a one-to-six-line auto-growing input.
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let textarea = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(1, 6)
                .placeholder("Ask your library…")
        });

        let input = cx.subscribe(&textarea, |this, _, event: &InputEvent, cx| match event {
            InputEvent::PressEnter {
                secondary: true, ..
            } => this.submit(cx),
            InputEvent::Change | InputEvent::Focus | InputEvent::Blur => cx.notify(),
            InputEvent::PressEnter { .. } => {}
        });

        Self {
            textarea,
            streaming: false,
            disabled: false,
            pending_clear: false,
            hint: None,
            send_icon: ChatIcon::Named(IconName::Send),
            _input: input,
        }
    }

    /// Replace the send button's mark.
    ///
    /// A raster [`ChatIcon`] has no monochrome form, so the send button falls
    /// back to the catalog `Send` icon.
    pub fn send_icon(mut self, icon: impl Into<ChatIcon>) -> Self {
        self.send_icon = icon.into();
        self
    }

    /// Replace the send button's mark and redraw.
    pub fn set_send_icon(&mut self, icon: impl Into<ChatIcon>, cx: &mut Context<Self>) {
        self.send_icon = icon.into();
        cx.notify();
    }

    /// The underlying textarea, for hosts that need selection or IME access.
    pub fn textarea(&self) -> &Entity<TextareaState> {
        &self.textarea
    }

    /// Current text, including untrimmed whitespace.
    pub fn value(&self, cx: &App) -> SharedString {
        self.textarea.read(cx).value()
    }

    /// Replace the draft and move the caret to the end.
    pub fn set_draft(
        &mut self,
        text: impl Into<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = text.into();
        self.textarea
            .update(cx, |state, cx| state.set_value(text, window, cx));
        self.textarea
            .update(cx, |state, cx| state.focus(window, cx));
    }

    /// Clear the draft.
    pub fn clear(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.textarea
            .update(cx, |state, cx| state.clean(window, cx));
    }

    /// Focus the input.
    pub fn focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.textarea
            .update(cx, |state, cx| state.focus(window, cx));
    }

    /// Switch between send and stop.
    pub fn set_streaming(&mut self, streaming: bool, cx: &mut Context<Self>) {
        if self.streaming != streaming {
            self.streaming = streaming;
            cx.notify();
        }
    }

    /// Returns whether the composer currently offers Stop.
    pub fn is_streaming(&self) -> bool {
        self.streaming
    }

    /// Disable submission and editing.
    pub fn set_disabled(&mut self, disabled: bool, cx: &mut Context<Self>) {
        if self.disabled != disabled {
            self.disabled = disabled;
            cx.notify();
        }
    }

    /// Override the line under the input.
    pub fn set_hint(&mut self, hint: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.hint = Some(hint.into());
        cx.notify();
    }

    /// Emit [`ComposerEvent::Submit`] for non-empty text.
    ///
    /// Called by Ctrl/⌘+Enter and by the Send button.
    pub fn submit(&mut self, cx: &mut Context<Self>) {
        if self.streaming || self.disabled {
            return;
        }
        let text = self.textarea.read(cx).value().trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.pending_clear = true;
        cx.emit(ComposerEvent::Submit(text));
        cx.notify();
    }

    /// Emit [`ComposerEvent::Stop`] while a generation is running.
    pub fn request_stop(&mut self, cx: &mut Context<Self>) {
        if self.streaming {
            cx.emit(ComposerEvent::Stop);
        }
    }

    /// Submit the current draft, or emit [`ComposerEvent::Stop`] while streaming.
    pub fn submit_or_stop(&mut self, cx: &mut Context<Self>) {
        if self.streaming {
            self.request_stop(cx);
        } else {
            self.submit(cx);
        }
    }
}

impl Render for Composer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.pending_clear) {
            self.textarea
                .update(cx, |state, cx| state.clean(window, cx));
            self.textarea
                .update(cx, |state, cx| state.focus(window, cx));
        }

        let empty = self.textarea.read(cx).value().trim().is_empty();
        let entity_id = cx.entity_id().as_u64();
        let action_id = ElementId::NamedInteger("composer-action".into(), entity_id);

        let action = if self.streaming {
            Button::new(action_id)
                .icon(IconName::Square)
                .label("Stop")
                .small()
                .on_click(cx.listener(|this, _, _, cx| this.submit_or_stop(cx)))
        } else {
            Button::new(action_id)
                .icon(
                    self.send_icon
                        .as_icon()
                        .unwrap_or_else(|| Icon::new(IconName::Send)),
                )
                .label("Send")
                .primary()
                .small()
                .disabled(empty || self.disabled)
                .on_click(cx.listener(|this, _, _, cx| this.submit_or_stop(cx)))
        };

        let hint = self.hint.clone().unwrap_or_else(|| submit_hint().into());

        v_flex()
            .p_3()
            .gap_2()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(Textarea::new(&self.textarea).bordered(true))
            .child(
                h_flex()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child(hint),
                    )
                    .child(action),
            )
    }
}

impl EventEmitter<ComposerEvent> for Composer {}

impl Focusable for Composer {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.textarea.read(cx).focus_handle(cx)
    }
}

fn submit_hint() -> &'static str {
    if cfg!(target_os = "macos") {
        "⌘+Enter to send · Enter for a new line"
    } else {
        "Ctrl+Enter to send · Enter for a new line"
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui_kit::{Render, TestAppContext, VisualTestContext};

    use super::*;

    struct Host {
        composer: Entity<Composer>,
        events: Rc<RefCell<Vec<ComposerEvent>>>,
    }

    impl Host {
        fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
            let composer = cx.new(|cx| Composer::new(window, cx));
            let events = Rc::new(RefCell::new(Vec::new()));
            let sink = events.clone();
            cx.subscribe(&composer, move |_, _, event: &ComposerEvent, _| {
                sink.borrow_mut().push(event.clone());
            })
            .detach();
            Self { composer, events }
        }
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            self.composer.clone()
        }
    }

    fn mount(cx: &mut TestAppContext) -> (Entity<Host>, &mut VisualTestContext) {
        cx.update(crate::init);
        cx.add_window_view(Host::new)
    }

    fn focus(host: &Entity<Host>, cx: &mut VisualTestContext) {
        cx.update(|window, cx| {
            host.update(cx, |host, cx| {
                host.composer
                    .update(cx, |composer, cx| composer.focus(window, cx));
            });
        });
    }

    #[gpui_kit::test]
    fn secondary_enter_emits_submit(cx: &mut TestAppContext) {
        let (host, cx) = mount(cx);
        focus(&host, cx);
        cx.simulate_input("Summarise the corpus");

        let shortcut = if cfg!(target_os = "macos") {
            "cmd-enter"
        } else {
            "ctrl-enter"
        };
        cx.simulate_keystrokes(shortcut);

        cx.update(|_, cx| {
            assert_eq!(
                host.read(cx).events.borrow().as_slice(),
                &[ComposerEvent::Submit("Summarise the corpus".to_owned())]
            );
        });
    }

    #[gpui_kit::test]
    fn enter_inserts_a_newline_without_submitting(cx: &mut TestAppContext) {
        let (host, cx) = mount(cx);
        focus(&host, cx);
        cx.simulate_input("hello");
        cx.simulate_keystrokes("enter");

        cx.update(|_, cx| {
            assert_eq!(host.read(cx).composer.read(cx).value(cx), "hello\n");
            assert!(host.read(cx).events.borrow().is_empty());
        });
    }

    #[gpui_kit::test]
    fn a_streaming_composer_emits_stop(cx: &mut TestAppContext) {
        let (host, cx) = mount(cx);
        cx.update(|_, cx| {
            host.update(cx, |host, cx| {
                host.composer.update(cx, |composer, cx| {
                    composer.set_streaming(true, cx);
                    composer.submit_or_stop(cx);
                });
            });
        });

        cx.update(|_, cx| {
            assert_eq!(
                host.read(cx).events.borrow().as_slice(),
                &[ComposerEvent::Stop]
            );
        });
    }
}
