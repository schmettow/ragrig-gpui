//! Composer attachments.

use gpui_kit::assets::IconName;

/// How an attachment chip behaves.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    /// A context toggle, rendered selected while `on`.
    Toggle {
        /// Current state.
        on: bool,
    },
    /// Context waiting for the next question; it can be dropped.
    Pending,
    /// A ready artefact with a host-named action, plus a drop button.
    Offer {
        /// Action caption, e.g. `Insert`.
        action: String,
    },
}

/// One chip above the composer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attachment {
    /// Stable identifier reported back on interaction.
    pub id: String,
    /// What the chip offers.
    pub kind: AttachmentKind,
    /// Icon shown before the label.
    pub icon: IconName,
    /// Chip text, e.g. `draft.md`.
    pub label: String,
    /// Optional tooltip.
    pub hint: Option<String>,
    /// Whether the chip accepts interaction.
    pub enabled: bool,
}

impl Attachment {
    /// A document-context toggle, initially off.
    pub fn document(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: AttachmentKind::Toggle { on: false },
            icon: IconName::FileText,
            label: label.into(),
            hint: None,
            enabled: true,
        }
    }

    /// A bibliography-context toggle, initially off.
    pub fn bibliography(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: AttachmentKind::Toggle { on: false },
            icon: IconName::BookOpen,
            label: label.into(),
            hint: None,
            enabled: true,
        }
    }

    /// Context that waits for the next question.
    pub fn pending(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind: AttachmentKind::Pending,
            icon: IconName::Paperclip,
            label: label.into(),
            hint: None,
            enabled: true,
        }
    }

    /// A ready artefact with an action button.
    pub fn offer(
        id: impl Into<String>,
        label: impl Into<String>,
        action: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            kind: AttachmentKind::Offer {
                action: action.into(),
            },
            icon: IconName::FileText,
            label: label.into(),
            hint: None,
            enabled: true,
        }
    }

    /// Set the toggle state (meaningful for [`AttachmentKind::Toggle`]).
    pub fn on(mut self, on: bool) -> Self {
        if let AttachmentKind::Toggle { on: state } = &mut self.kind {
            *state = on;
        }
        self
    }

    /// Set the tooltip.
    pub fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    /// Enable or disable the whole chip.
    pub fn enabled(mut self, enabled: bool) -> Self {
        self.enabled = enabled;
        self
    }

    /// Returns whether this is a selected toggle.
    pub fn is_on(&self) -> bool {
        matches!(self.kind, AttachmentKind::Toggle { on: true })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_state_is_readable_and_settable() {
        let attachment = Attachment::document("draft", "draft.md");
        assert!(!attachment.is_on());
        assert!(attachment.clone().on(true).is_on());
    }

    #[test]
    fn kinds_carry_their_own_action() {
        let offer = Attachment::offer("refs", "references.bib", "Insert");
        assert_eq!(
            offer.kind,
            AttachmentKind::Offer {
                action: "Insert".to_owned()
            }
        );
        assert_eq!(
            Attachment::pending("region", "Selection").kind,
            AttachmentKind::Pending
        );
    }
}
