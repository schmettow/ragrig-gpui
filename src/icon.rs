//! Icons supplied by the host for chat chrome.
//!
//! The transcript and the empty state are reusable, so they cannot assume a
//! particular application icon. A [`ChatIcon`] is either a catalog name, a path
//! in the application's asset source, or SVG bytes compiled into the binary.

use std::{fmt, sync::Arc};

use gpui_kit::{SharedString, assets::IconName, component::Icon};

/// An icon the host can plug into [`crate::EmptyState`] or
/// [`crate::ChatIdentity`].
#[derive(Clone)]
pub enum ChatIcon {
    /// A named icon resolved through the application's asset source.
    Named(IconName),
    /// A path in the application's asset source, e.g. `logos/scico.svg`.
    Path(SharedString),
    /// SVG bytes carried directly by the element.
    ///
    /// This needs no asset registration, which suits a logo that lives in the
    /// binary: `ChatIcon::data(include_bytes!("../assets/logo.svg"))`.
    Data(Arc<[u8]>),
}

impl ChatIcon {
    /// Wrap SVG bytes. The bytes are copied once into shared storage and then
    /// reused across renders.
    pub fn data(bytes: impl AsRef<[u8]>) -> Self {
        Self::Data(Arc::from(bytes.as_ref()))
    }

    /// Wrap a path in the application's asset source.
    pub fn path(path: impl Into<SharedString>) -> Self {
        Self::Path(path.into())
    }

    /// Build the renderable icon. Size and colour are inherited from the
    /// surrounding text style.
    pub(crate) fn view(&self) -> Icon {
        match self {
            Self::Named(name) => Icon::new(*name),
            Self::Path(path) => Icon::default().path(path.clone()),
            Self::Data(bytes) => Icon::default().data(bytes),
        }
    }
}

impl From<IconName> for ChatIcon {
    fn from(name: IconName) -> Self {
        Self::Named(name)
    }
}

impl Default for ChatIcon {
    fn default() -> Self {
        Self::Named(IconName::Sparkles)
    }
}

impl fmt::Debug for ChatIcon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Named(name) => f.debug_tuple("Named").field(name).finish(),
            Self::Path(path) => f.debug_tuple("Path").field(path).finish(),
            Self::Data(bytes) => f
                .debug_tuple("Data")
                .field(&format_args!("{} bytes", bytes.len()))
                .finish(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::SvgRenderer;

    const SVG: &[u8] = br#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 16 16">
        <circle cx="8" cy="8" r="7" fill="currentColor"/>
    </svg>"#;

    #[test]
    fn inline_svg_icons_render_visible_pixels() {
        let icon = ChatIcon::data(SVG);
        let ChatIcon::Data(bytes) = &icon else {
            panic!("expected inline data, got {icon:?}");
        };
        let renderer = SvgRenderer::new(Arc::new(gpui_kit::assets::AllAssets));
        let parsed = renderer.parse_svg(bytes).expect("parse inline svg");
        let image = renderer.render_parsed(&parsed, 2.0).expect("render svg");
        let pixels = image.as_bytes(0).expect("frame");
        assert!(
            pixels.as_chunks::<4>().0.iter().any(|pixel| pixel[3] != 0),
            "inline SVG rasterised to a blank image"
        );
    }

    #[test]
    fn a_catalog_icon_converts() {
        let icon: ChatIcon = IconName::Bot.into();
        assert!(matches!(icon, ChatIcon::Named(IconName::Bot)));
    }
}
