//! Agent pipeline components and their status.

use gpui_kit::assets::IconName;

use crate::icon::ChatIcon;

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
    /// Conversation memory and query rewriting.
    Memory,
    /// A vision or OCR component.
    Vision,
    /// Bibliographic reference extraction.
    References,
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
            Self::Memory => "Chat memory",
            Self::Vision => "Vision",
            Self::References => "References",
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
            Self::Chunker => IconName::Pilcrow,
            Self::Retriever => IconName::Search,
            Self::Memory => IconName::Brain,
            Self::Vision => IconName::Eye,
            Self::References => IconName::BookOpen,
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

/// A switch shown in a component's status popover.
///
/// Components that can be turned on and off expose one or more of these.
/// The bar reports a change through `AgentBarEvent::Toggle`; the host owns
/// the actual setting and reflects the new value in the component on the next
/// render.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComponentToggle {
    /// Stable identifier reported when the switch changes.
    pub id: String,
    /// Switch label, e.g. `Enabled`.
    pub label: String,
    /// Whether the aspect is currently on.
    pub enabled: bool,
    /// Optional explanation shown under the label.
    pub detail: Option<String>,
}

impl ComponentToggle {
    /// A switch with an ID, label, and current state.
    pub fn new(id: impl Into<String>, label: impl Into<String>, enabled: bool) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            enabled,
            detail: None,
        }
    }

    /// Describe what switching the aspect on or off means.
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
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
    /// Optional mark overriding the kind's catalog icon.
    pub icon: Option<ChatIcon>,
    /// Switches offered in the component popover.
    pub toggles: Vec<ComponentToggle>,
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
            icon: None,
            toggles: Vec::new(),
        }
    }

    /// Replace the catalog icon with a host-supplied mark.
    ///
    /// Buttons need a monochrome form; a raster mark falls back to the kind's
    /// catalog icon.
    pub fn icon(mut self, icon: impl Into<ChatIcon>) -> Self {
        self.icon = Some(icon.into());
        self
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

    /// Add a switch to the component popover.
    pub fn toggle(mut self, toggle: ComponentToggle) -> Self {
        self.toggles.push(toggle);
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
        assert_eq!(ComponentKind::Chunker.icon(), IconName::Pilcrow);
        assert_eq!(ComponentKind::References.label(), "References");
        assert_eq!(ComponentKind::References.icon(), IconName::BookOpen);
    }

    #[test]
    fn every_component_icon_draws_visible_pixels() {
        use gpui_kit::{AssetSource as _, SvgRenderer};
        use std::sync::Arc;

        let assets = Arc::new(gpui_kit::assets::AllAssets);
        let renderer = SvgRenderer::new(assets.clone());
        let kinds = [
            ComponentKind::Chat,
            ComponentKind::Embedder,
            ComponentKind::Store,
            ComponentKind::Parser,
            ComponentKind::Chunker,
            ComponentKind::Retriever,
            ComponentKind::Memory,
            ComponentKind::Vision,
            ComponentKind::References,
            ComponentKind::Other,
        ];
        for kind in kinds {
            let path = kind.icon().path();
            let bytes = assets.load(&path).expect("asset load").expect("asset");
            let parsed = renderer.parse_svg(&bytes).expect("parse svg");
            let image = renderer.render_parsed(&parsed, 2.0f32).expect("render svg");
            let pixels = image.as_bytes(0).expect("frame");
            assert!(
                pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0),
                "{:?} ({path}) rasterised to a blank image",
                kind
            );
        }
    }

    #[test]
    fn components_default_to_inactive() {
        let component = AgentComponent::new("chat", ComponentKind::Chat, "DeepSeek");
        assert_eq!(component.status, ComponentStatus::Inactive);
        assert!(!component.status.is_busy());
        assert!(component.toggles.is_empty());
    }

    #[test]
    fn a_component_can_carry_switches() {
        let component = AgentComponent::new("memory", ComponentKind::Memory, "Chat memory")
            .toggle(ComponentToggle::new("memory", "Enabled", true).detail("rewrite"));
        assert_eq!(component.toggles.len(), 1);
        assert_eq!(component.toggles[0].id, "memory");
        assert!(component.toggles[0].enabled);
    }
}
