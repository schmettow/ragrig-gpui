//! Saved conversations shown in the session picker.

/// One saved conversation.
///
/// The host owns the store and formats `label`; the picker only reports the
/// `id` back. This keeps date formatting and localization out of the library.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionEntry {
    /// Identifier the host uses to load the conversation.
    pub id: String,
    /// Preformatted row text, e.g. `12 turns · about the methods section`.
    pub label: String,
    /// Optional second line, e.g. a timestamp.
    pub detail: Option<String>,
}

impl SessionEntry {
    /// A session row with a label and no detail line.
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
        }
    }

    /// Set the second line.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_session_carries_an_id_and_preformatted_text() {
        let session = SessionEntry::new("s1", "3 turns").detail("yesterday");
        assert_eq!(session.id, "s1");
        assert_eq!(session.label, "3 turns");
        assert_eq!(session.detail.as_deref(), Some("yesterday"));
    }
}
