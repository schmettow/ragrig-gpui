//! Icons supplied by the host for chat chrome.
//!
//! The transcript and the empty state are reusable, so they cannot assume a
//! particular application icon. A [`ChatIcon`] is either a catalog name, a path
//! in the application's asset source, or SVG bytes compiled into the binary.

use std::{
    collections::hash_map::DefaultHasher,
    fmt,
    hash::{Hash as _, Hasher as _},
    sync::Arc,
};

use gpui_kit::{
    AnyElement, Image, ImageFormat, ImageSource, IntoElement, Pixels, SharedString, Styled as _,
    assets::IconName, component::Icon, img, px,
};

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
    /// Full-colour PNG bytes.
    ///
    /// GPUI paints SVG icons as a monochrome alpha mask. A multi-colour mark
    /// must therefore be a raster image; [`Self::png`] builds one.
    Png(Arc<Image>),
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

    /// Wrap full-colour PNG bytes for an image element.
    pub fn png(bytes: impl AsRef<[u8]>) -> Self {
        let bytes = bytes.as_ref();
        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        Self::Png(Arc::new(Image {
            format: ImageFormat::Png,
            bytes: bytes.to_vec(),
            id: hasher.finish(),
        }))
    }

    /// The monochrome icon, when this mark is an SVG icon.
    ///
    /// Buttons need an [`Icon`]; a raster mark has no monochrome form, so the
    /// caller should fall back to a catalog icon.
    pub(crate) fn as_icon(&self) -> Option<Icon> {
        match self {
            Self::Named(name) => Some(Icon::new(*name)),
            Self::Path(path) => Some(Icon::default().path(path.clone())),
            Self::Data(bytes) => Some(Icon::default().data(bytes)),
            Self::Png(_) => None,
        }
    }

    /// Render the mark at an explicit size, or at the surrounding text size
    /// for SVG icons. Raster marks default to 24 px when no size is given.
    pub(crate) fn element(&self, size: Option<Pixels>) -> AnyElement {
        if let Self::Png(image) = self {
            return img(ImageSource::Image(image.clone()))
                .size(size.unwrap_or(px(24.)))
                .into_any_element();
        }

        let icon = self
            .as_icon()
            .unwrap_or_else(|| Icon::new(IconName::Sparkles));
        match size {
            Some(size) => icon.size(size).into_any_element(),
            None => icon.into_any_element(),
        }
    }
}

impl From<IconName> for ChatIcon {
    fn from(name: IconName) -> Self {
        Self::Named(name)
    }
}

impl PartialEq for ChatIcon {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Named(a), Self::Named(b)) => a == b,
            (Self::Path(a), Self::Path(b)) => a == b,
            (Self::Data(a), Self::Data(b)) => a == b,
            (Self::Png(a), Self::Png(b)) => a == b,
            _ => false,
        }
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
            Self::Png(image) => f
                .debug_tuple("Png")
                .field(&format_args!("{} bytes", image.bytes.len()))
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

    #[test]
    fn a_raster_mark_has_no_monochrome_form() {
        let icon = ChatIcon::png(b"not decoded here");
        assert!(matches!(icon, ChatIcon::Png(_)));
        assert!(icon.as_icon().is_none());
        assert_eq!(icon, ChatIcon::png(b"not decoded here"));
    }
}
