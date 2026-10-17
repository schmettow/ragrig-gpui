//! Agent pipeline components and their status.

use gpui_kit::assets::IconName;

/// The role a component plays in the RAG pipeline.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ComponentKind {
    /// The chat model.
    Chat,
    /// The embedding model.
    Embedder,
    /// The vector or document store.
    Store,
    /// The document parser.
    Parser,
    /// The chunker.
    Chunker,
    /// The retrieval strategy.
    Retriever,
    /// A vision or OCR component.
    Vision,
    /// Anything else.
    Other,
}

impl ComponentKind {
    /// Short human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Chat => "Chat model",
            Self::Embedder => "Embedder",
            Self::Store => "Store",
            Self::Parser => "Parser",
            Self::Chunker => "Chunker",
            Self::Retriever => "Retriever",
            Self::Vision => "Vision",
            Self::Other => "Component",
        }
    }

    /// Icon used in the agent bar.
    pub fn icon(self) -> IconName {
        match self {
            Self::Chat => IconName::Bot,
            Self::Embedder => IconName::Cpu,
            Self::Store => IconName::Database,
            Self::Parser => IconName::FileText,
            Self::Chunker => IconName::Scissors,
            Self::Retriever => IconName::Search,
            Self::Vision => IconName::Eye,
            Self::Other => IconName::Settings,
        }
    }
}

/// Status of one agent component.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ComponentStatus {
    /// Not configured or switched off.
    #[default]
    Inactive,
    /// Configured and available.
    Ready,
    /// Currently working.
    Busy,
    /// Failed; the detail line explains why.
    Error,
}

impl ComponentStatus {
    /// Short human-readable name.
    pub fn label(self) -> &'static str {
        match self {
            Self::Inactive => "inactive",
            Self::Ready => "ready",
            Self::Busy => "busy",
            Self::Error => "error",
        }
    }

    /// Returns whether the component deserves a pulsing dot.
    pub fn is_busy(self) -> bool {
        matches!(self, Self::Busy)
    }
}

/// One component shown in the agent bar.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentComponent {
    /// Stable identifier reported when the component is configured.
    pub id: String,
    /// Role in the pipeline.
    pub kind: ComponentKind,
    /// Display label, e.g. `DeepSeek Chat`.
    pub label: String,
    /// Current status.
    pub status: ComponentStatus,
    /// Optional detail line: backend, model, last error.
    pub detail: Option<String>,
}

impl AgentComponent {
    /// A component with an ID, kind, and label.
    pub fn new(id: impl Into<String>, kind: ComponentKind, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            kind,
            label: label.into(),
            status: ComponentStatus::default(),
            detail: None,
        }
    }

    /// Set the status.
    pub fn status(mut self, status: ComponentStatus) -> Self {
        self.status = status;
        self
    }

    /// Set the detail line.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_have_labels_and_icons() {
        assert_eq!(ComponentKind::Chat.label(), "Chat model");
        assert_eq!(ComponentKind::Store.icon(), IconName::Database);
    }

    #[test]
    fn components_default_to_inactive() {
        let component = AgentComponent::new("chat", ComponentKind::Chat, "DeepSeek");
        assert_eq!(component.status, ComponentStatus::Inactive);
        assert!(!component.status.is_busy());
    }
}
