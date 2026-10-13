//! Management of the Stylo [`Device`]: constructing it from shell state and
//! coalescing changes to it (viewport resizes, zoom, color-scheme and media
//! type changes) so that the stylist device is rebuilt at most once per
//! [`resolve`](crate::BaseDocument::resolve), no matter how many changes
//! occurred since the last resolve.

use std::sync::{Arc, Mutex};

use bitflags::bitflags;
use blitz_traits::shell::{ColorScheme, Viewport};
use parley::FontContext;
use selectors::matching::QuirksMode;
use style::device::Device;
use style::device::servo::LineWidthRounding;
use style::media_queries::MediaType;
use style::properties::ComputedValues;
use style::properties::style_structs::Font;
use style::queries::values::PrefersColorScheme;
use style::servo::media_features::{PointerCapabilities, PrefersContrast, PrefersReducedMotion};

use crate::font_metrics::BlitzFontMetricsProvider;

bitflags! {
    /// The set of changes to the [`Device`] that have accumulated since the
    /// stylist device was last rebuilt. Only the *latest* values matter (they
    /// live on [`BaseDocument`](crate::BaseDocument)); these flags record
    /// which kinds of change occurred so that the flush can do the right
    /// amount of invalidation work.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
    pub(crate) struct DeviceChanges: u8 {
        /// The CSS pixel viewport size changed (window resize, zoom, or hidpi
        /// scale change).
        const VIEWPORT_SIZE = 0b0000_0001;
        /// The total scale factor (hidpi scale * zoom) changed. Requires
        /// invalidating cached inline layouts (text is shaped at a specific
        /// scale) and a redraw.
        const SCALE = 0b0000_0010;
        /// The preferred color scheme changed.
        const COLOR_SCHEME = 0b0000_0100;
        /// The media type (e.g. screen/print) changed.
        const MEDIA_TYPE = 0b0000_1000;
        /// The document's compatibility mode changed.
        const QUIRKS_MODE = 0b0001_0000;
        /// The user preferences media queries see changed.
        const MEDIA_PREFERENCES = 0b0010_0000;
    }
}

impl DeviceChanges {
    /// Compute the [`DeviceChanges`] implied by moving from viewport `old` to
    /// viewport `new`.
    pub(crate) fn from_viewports(old: &Viewport, new: &Viewport) -> Self {
        let mut changes = Self::empty();
        if old.logical_size() != new.logical_size() {
            changes |= Self::VIEWPORT_SIZE;
        }
        if old.scale_f64() != new.scale_f64() {
            changes |= Self::SCALE;
        }
        if old.color_scheme != new.color_scheme {
            changes |= Self::COLOR_SCHEME;
        }
        changes
    }
}

/// The user preferences `@media` rules see beside the color scheme (which
/// is part of the [`Viewport`]). The default is no preference, what a
/// browser reports without a desktop setting for them.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MediaPreferences {
    /// `prefers-reduced-motion`.
    pub reduced_motion: PrefersReducedMotion,
    /// `prefers-contrast`.
    pub contrast: PrefersContrast,
}

impl MediaPreferences {
    pub(crate) fn apply(&self, device: &mut Device) {
        device.set_prefers_reduced_motion(self.reduced_motion);
        device.set_prefers_contrast(self.contrast);
    }
}

pub(crate) fn make_device(
    viewport: &Viewport,
    media_type: MediaType,
    quirks_mode: QuirksMode,
    font_ctx: Arc<Mutex<FontContext>>,
) -> Device {
    let (width, height) = viewport.logical_size();
    let viewport_size = euclid::Size2D::new(width, height);
    let device_size = euclid::Size2D::new(width, height) * viewport.scale();
    let device_pixel_ratio = euclid::Scale::new(viewport.scale());

    Device::new(
        media_type,
        quirks_mode,
        viewport_size,
        device_size,
        device_pixel_ratio,
        Box::new(BlitzFontMetricsProvider { font_ctx }),
        ComputedValues::initial_values_with_font_override(Font::initial_values()),
        match viewport.color_scheme {
            ColorScheme::Light => PrefersColorScheme::Light,
            ColorScheme::Dark => PrefersColorScheme::Dark,
        },
        PointerCapabilities::default(),
        PointerCapabilities::default(),
    )
    .with_line_width_rounding(LineWidthRounding::CssPixel)
}
