//! Conversation picker.

use std::rc::Rc;

use gpui_kit::{
    Anchor, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _, RenderOnce,
    SharedString, Styled as _, Window,
    assets::IconName,
    component::{
        ActiveTheme as _, Disableable as _, Selectable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        popover::Popover,
        scroll::ScrollableElement as _,
        v_flex,
    },
    div, px,
};

use crate::model::SessionEntry;

/// What the user asked the picker to do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionPickerEvent {
    /// Start a fresh conversation.
    New,
    /// Load the conversation with this id.
    Load(String),
    /// Re-read the session store.
    Refresh,
}

type SessionHandler = Rc<dyn Fn(SessionPickerEvent, &mut Window, &mut App)>;

/// A dropdown of saved conversations.
///
/// The host owns the store: it passes [`SessionEntry`] rows and reacts to
/// [`SessionPickerEvent`]. The current entry is highlighted; selecting one
/// emits [`SessionPickerEvent::Load`] and closes the popover.
#[derive(IntoElement)]
pub struct SessionPicker {
    id: SharedString,
    entries: Vec<SessionEntry>,
    current: Option<String>,
    busy: bool,
    on_event: Option<SessionHandler>,
}

impl SessionPicker {
    /// A picker with a unique element-ID prefix.
    pub fn new(
        id: impl Into<SharedString>,
        entries: impl IntoIterator<Item = SessionEntry>,
    ) -> Self {
        Self {
            id: id.into(),
            entries: entries.into_iter().collect(),
            current: None,
            busy: false,
            on_event: None,
        }
    }

    /// Mark the active conversation.
    pub fn current(mut self, id: impl Into<String>) -> Self {
        self.current = Some(id.into());
        self
    }

    /// Disable creating and refreshing while the host is busy.
    pub fn busy(mut self, busy: bool) -> Self {
        self.busy = busy;
        self
    }

    /// Receive picker actions.
    pub fn on_event(
        mut self,
        handler: impl Fn(SessionPickerEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for SessionPicker {
    fn render(self, _: &mut Window, _cx: &mut App) -> impl IntoElement {
        let current_label = self
            .entries
            .iter()
            .find(|entry| Some(entry.id.as_str()) == self.current.as_deref())
            .map_or_else(|| "Conversation".to_owned(), |entry| entry.label.clone());

        let trigger_id = ElementId::Name(format!("{}-trigger", self.id).into());
        let popover_id = ElementId::Name(format!("{}-popover", self.id).into());
        let list_id = ElementId::Name(format!("{}-list", self.id).into());

        let entries = self.entries;
        let current = self.current;
        let busy = self.busy;
        let handler = self.on_event;
        let prefix = self.id;

        Popover::new(popover_id)
            .anchor(Anchor::TopLeft)
            .trigger(
                Button::new(trigger_id)
                    .icon(IconName::MessagesSquare)
                    .label(current_label)
                    .outline()
                    .small()
                    .tooltip("Switch conversation"),
            )
            .content(move |_, _, cx| {
                let popover = cx.entity();
                let mut list = v_flex().w(px(320.)).p_1().gap_1();

                let new_handler = handler.clone();
                let new_popover = popover.clone();
                let new_id = ElementId::Name(format!("{prefix}-new").into());
                list = list.child(
                    Button::new(new_id)
                        .icon(IconName::Plus)
                        .label("New conversation")
                        .ghost()
                        .small()
                        .w_full()
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            if let Some(handler) = &new_handler {
                                handler(SessionPickerEvent::New, window, cx);
                            }
                            new_popover.update(cx, |state, cx| state.dismiss(window, cx));
                        }),
                );

                let mut entries_list = v_flex().id(list_id.clone()).gap_1();
                if entries.is_empty() {
                    entries_list = entries_list.child(
                        div()
                            .px_2()
                            .py_2()
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("No saved conversations yet."),
                    );
                } else {
                    for (index, entry) in entries.iter().cloned().enumerate() {
                        let is_current = Some(entry.id.as_str()) == current.as_deref();
                        let entry_handler = handler.clone();
                        let entry_popover = popover.clone();
                        let entry_id = entry.id.clone();
                        let button_id = ElementId::Name(format!("{prefix}-entry-{index}").into());
                        let label = entry.label.clone();
                        let detail = entry.detail.clone();

                        let label: SharedString = if let Some(detail) = detail {
                            format!("{label} · {detail}").into()
                        } else {
                            label.into()
                        };

                        let mut button = Button::new(button_id)
                            .icon(if is_current {
                                IconName::Check
                            } else {
                                IconName::MessageSquare
                            })
                            .label(label)
                            .ghost()
                            .small()
                            .selected(is_current)
                            .w_full()
                            .on_click(move |_, window, cx| {
                                if let Some(handler) = &entry_handler {
                                    handler(SessionPickerEvent::Load(entry_id.clone()), window, cx);
                                }
                                entry_popover.update(cx, |state, cx| state.dismiss(window, cx));
                            });
                        if is_current {
                            button = button.primary();
                        }
                        entries_list = entries_list.child(button);
                    }
                }
                list = list.child(entries_list.max_h(px(360.)).overflow_y_scrollbar());

                let refresh_handler = handler.clone();
                let refresh_popover = popover.clone();
                let refresh_id = ElementId::Name(format!("{prefix}-refresh").into());
                list.child(
                    Button::new(refresh_id)
                        .icon(IconName::RefreshCw)
                        .label("Refresh list")
                        .ghost()
                        .small()
                        .w_full()
                        .disabled(busy)
                        .on_click(move |_, window, cx| {
                            if let Some(handler) = &refresh_handler {
                                handler(SessionPickerEvent::Refresh, window, cx);
                            }
                            refresh_popover.update(cx, |state, cx| state.dismiss(window, cx));
                        }),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_collected_and_current_is_optional() {
        let picker = SessionPicker::new(
            "sessions",
            [
                SessionEntry::new("a", "First"),
                SessionEntry::new("b", "Second"),
            ],
        )
        .current("b");

        assert_eq!(picker.entries.len(), 2);
        assert_eq!(picker.current.as_deref(), Some("b"));
    }
}
