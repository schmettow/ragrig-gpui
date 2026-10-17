//! Composer attachment chips.

use std::rc::Rc;

use gpui_kit::{
    AnyElement, App, ElementId, InteractiveElement as _, IntoElement, ParentElement as _,
    RenderOnce, StatefulInteractiveElement as _, Styled as _, Window,
    assets::IconName,
    component::{
        ActiveTheme as _, Disableable as _, Sizable as _,
        button::{Button, ButtonVariants as _},
        h_flex,
    },
    div,
};

use crate::model::{Attachment, AttachmentKind};

/// What the user did with an attachment chip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentEvent {
    /// A toggle chip was clicked; the host flips its state.
    Toggled(String),
    /// The drop button was clicked.
    Dropped(String),
    /// An offer's action button was clicked.
    Accepted(String),
}

type AttachmentHandler = Rc<dyn Fn(AttachmentEvent, &mut Window, &mut App)>;

/// A wrapping row of [`Attachment`] chips.
///
/// The builder is presentational: it reports interactions through
/// [`on_event`](Self::on_event) and never mutates the attachments.
#[derive(IntoElement)]
pub struct AttachmentRows {
    attachments: Vec<Attachment>,
    on_event: Option<AttachmentHandler>,
}

impl AttachmentRows {
    /// Render `attachments` left to right, wrapping when needed.
    pub fn new(attachments: impl IntoIterator<Item = Attachment>) -> Self {
        Self {
            attachments: attachments.into_iter().collect(),
            on_event: None,
        }
    }

    /// Receive chip interactions.
    pub fn on_event(
        mut self,
        handler: impl Fn(AttachmentEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_event = Some(Rc::new(handler));
        self
    }
}

impl RenderOnce for AttachmentRows {
    fn render(self, _: &mut Window, cx: &mut App) -> impl IntoElement {
        let handler = self.on_event;
        h_flex().flex_wrap().gap_2().children(
            self.attachments
                .into_iter()
                .enumerate()
                .map(|(index, attachment)| chip(index, attachment, handler.clone(), cx)),
        )
    }
}

fn chip(
    index: usize,
    attachment: Attachment,
    handler: Option<AttachmentHandler>,
    cx: &mut App,
) -> AnyElement {
    let theme = cx.theme();
    let on = attachment.is_on();
    let enabled = attachment.enabled;
    let id = attachment.id.clone();
    let element_id = ElementId::Name(format!("attachment-{index}-{id}").into());

    let mut row = h_flex()
        .id(element_id)
        .items_center()
        .gap_1()
        .px_2()
        .py_1()
        .rounded(theme.radius)
        .border_1()
        .border_color(if on { theme.primary } else { theme.border })
        .bg(if on {
            theme.primary.opacity(0.08)
        } else {
            theme.background
        })
        .text_sm()
        .text_color(if enabled {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .child(
            div()
                .text_color(if on {
                    theme.primary
                } else {
                    theme.muted_foreground
                })
                .child(attachment.icon),
        )
        .child(attachment.label.clone());

    match &attachment.kind {
        AttachmentKind::Toggle { .. } => {
            if enabled {
                let handler = handler.clone();
                let id = id.clone();
                row = row.cursor_pointer().on_click(move |_, window, cx| {
                    if let Some(handler) = &handler {
                        handler(AttachmentEvent::Toggled(id.clone()), window, cx);
                    }
                });
            }
        }
        AttachmentKind::Pending => {
            row = row.child(drop_button(index, &id, enabled, handler.clone()));
        }
        AttachmentKind::Offer { action } => {
            let action_id = ElementId::Name(format!("attachment-offer-{index}-{id}").into());
            let accept_handler = handler.clone();
            let accept_id = id.clone();
            row = row
                .child(
                    Button::new(action_id)
                        .label(action.clone())
                        .xsmall()
                        .ghost()
                        .disabled(!enabled)
                        .on_click(move |_, window, cx| {
                            if let Some(handler) = &accept_handler {
                                handler(AttachmentEvent::Accepted(accept_id.clone()), window, cx);
                            }
                        }),
                )
                .child(drop_button(index, &id, enabled, handler));
        }
    }

    row.into_any_element()
}

fn drop_button(
    index: usize,
    id: &str,
    enabled: bool,
    handler: Option<AttachmentHandler>,
) -> Button {
    let element_id = ElementId::Name(format!("attachment-drop-{index}-{id}").into());
    let drop_id = id.to_owned();
    Button::new(element_id)
        .icon(IconName::Close)
        .xsmall()
        .ghost()
        .disabled(!enabled)
        .tooltip("Drop")
        .on_click(move |_, window, cx| {
            if let Some(handler) = &handler {
                handler(AttachmentEvent::Dropped(drop_id.clone()), window, cx);
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_name_the_attachment() {
        assert_eq!(
            AttachmentEvent::Toggled("draft".to_owned()),
            AttachmentEvent::Toggled("draft".to_owned())
        );
        assert_ne!(
            AttachmentEvent::Dropped("a".to_owned()),
            AttachmentEvent::Accepted("a".to_owned())
        );
    }

    #[test]
    fn a_row_accepts_any_iterator() {
        let rows = AttachmentRows::new([
            Attachment::document("draft", "draft.md"),
            Attachment::pending("region", "Selection"),
        ]);
        assert_eq!(rows.attachments.len(), 2);
    }
}
