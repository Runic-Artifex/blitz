use super::ElementCx;
use crate::color::{Color, ToColorColor as _};
use anyrender::PaintScene;
use blitz_dom::local_name;
use kurbo::{Affine, BezPath, Cap, Circle, Join, Point, RoundedRect, Stroke};
use peniko::Fill;
use style::computed_value_flags::ComputedValueFlags;
use style::dom::TElement as _;
use style::values::computed::Appearance;

impl ElementCx<'_, '_> {
    pub(super) fn is_native_checkable_input(&self) -> bool {
        self.node.local_name() == "input"
            && matches!(
                self.node.attr(local_name!("type")),
                Some("checkbox" | "radio")
            )
            && self.style.get_box().clone_appearance() != Appearance::None
    }

    /// Blink's native text controls retain their CSS box metrics, but paint a
    /// one-device-pixel widget frame unless the author supplies a border or
    /// background. Keep the author path in the ordinary CSS painters so
    /// `appearance: none` and author decoration remain conventional boxes.
    pub(super) fn has_native_text_control_frame(&self) -> bool {
        self.text_input.is_some()
            && self.style.get_box().clone_appearance() != Appearance::None
            && !self
                .style
                .flags
                .contains(ComputedValueFlags::HAS_AUTHOR_SPECIFIED_BORDER_BACKGROUND)
    }

    pub(super) fn draw_native_text_control_frame(&self, scene: &mut impl PaintScene) {
        if !self.has_native_text_control_frame() {
            return;
        }

        // Chrome's Linux theme leaves the second CSS border pixel as widget
        // interior, while its visual frame is one #767676 pixel with a two
        // pixel corner radius. The CSS border still participates in layout.
        let outer = self.frame.border_box.to_rounded_rect(2.0);
        let inner = self.frame.border_box.inset(-1.0).to_rounded_rect(1.0);
        scene.fill(
            Fill::NonZero,
            self.transform,
            Color::from_rgba8(0x76, 0x76, 0x76, 255),
            None,
            &outer,
        );
        scene.fill(Fill::NonZero, self.transform, Color::WHITE, None, &inner);
    }

    pub(super) fn draw_input(&self, scene: &mut impl PaintScene) {
        if !self.is_native_checkable_input() {
            return;
        }
        let Some(checked) = self.element.checkbox_input_checked() else {
            return;
        };

        let type_attr = self.node.attr(local_name!("type"));
        let disabled = self.node.attr(local_name!("disabled")).is_some();

        // Chromium paints checkboxes and radio buttons in fixed colours, not
        // the element's `color`: a #767676 border (#4f4f4f hovered), filled
        // with its accent colour (#0075ff, #005cc8 hovered; `accent-color:
        // auto`, which Stylo's servo mode does not parse) when checked.
        let hovered = self.node.is_hovered();
        let (accent_color, border_color) = if disabled {
            let grey = Color::from_rgba8(209, 209, 209, 255);
            (grey, grey)
        } else if hovered {
            (
                Color::from_rgba8(0x00, 0x5c, 0xc8, 255),
                Color::from_rgba8(0x4f, 0x4f, 0x4f, 255),
            )
        } else {
            (
                Color::from_rgba8(0x00, 0x75, 0xff, 255),
                Color::from_rgba8(0x76, 0x76, 0x76, 255),
            )
        };

        let width = self.frame.border_box.width();
        let height = self.frame.border_box.height();
        let min_dimension = width.min(height);
        let scale = (min_dimension - 4.0).max(0.0) / 16.0;
        // Native checkables paint a centred square widget within an author
        // supplied rectangular border box. Their author padding, border,
        // background and radius remain computed but are not the widget frame.
        let center = self.frame.border_box.center();
        let half = min_dimension / 2.0;
        let widget_rect = kurbo::Rect::new(
            center.x - half,
            center.y - half,
            center.x + half,
            center.y + half,
        );
        let frame = widget_rect.to_rounded_rect(2.0);

        match type_attr {
            Some("checkbox") => {
                draw_checkbox(
                    scene,
                    checked,
                    frame,
                    self.transform,
                    accent_color,
                    border_color,
                    scale,
                );
            }
            Some("radio") => {
                let center = frame.center();
                draw_radio_button(
                    scene,
                    checked,
                    center,
                    self.transform,
                    accent_color,
                    border_color,
                    min_dimension,
                );
            }
            _ => {}
        }
    }
}

fn draw_checkbox(
    scene: &mut impl PaintScene,
    checked: bool,
    frame: RoundedRect,
    transform: Affine,
    accent_color: Color,
    border_color: Color,
    scale: f64,
) {
    if checked {
        scene.fill(Fill::NonZero, transform, accent_color, None, &frame);
        // NativeThemeBase::PaintCheckbox expresses the tick as fractions of
        // its centred square, with a 0.16-height stroke.
        let rect = frame.rect();
        let mut path = BezPath::new();
        path.move_to((rect.x0 + rect.width() * 0.2, rect.center().y));
        path.line_to((rect.x0 + rect.width() * 0.4, rect.y0 + rect.height() * 0.7));
        path.line_to((rect.x1 - rect.width() * 0.2, rect.y0 + rect.height() * 0.2));

        let style = Stroke {
            width: rect.height() * 0.16,
            join: Join::Miter,
            miter_limit: 10.0,
            start_cap: Cap::Butt,
            end_cap: Cap::Butt,
            dash_pattern: Default::default(),
            dash_offset: 0.0,
        };

        scene.stroke(&style, transform, Color::WHITE, None, &path);
    } else {
        // A one pixel border inside the box (a stroke centred on its edge
        // would straddle pixels and paint lighter).
        let inset = frame.rect().inset(-scale.max(1.0));
        let inner = inset.to_rounded_rect((frame.radii().top_left - scale).max(0.0));
        scene.fill(Fill::NonZero, transform, border_color, None, &frame);
        scene.fill(Fill::NonZero, transform, Color::WHITE, None, &inner);
    }
}

fn draw_radio_button(
    scene: &mut impl PaintScene,
    checked: bool,
    center: Point,
    transform: Affine,
    accent_color: Color,
    border_color: Color,
    diameter: f64,
) {
    // NativeThemeBase::PaintCheckboxRadioCommon fills a square inset by 0.2
    // then strokes its centred circle one pixel wide. A checked radio adds a
    // 20%-inset accent dot; it does not use the former two-pixel ring model.
    let background = Circle::new(center, (diameter / 2.0 - 0.2).max(0.0));
    scene.fill(Fill::NonZero, transform, Color::WHITE, None, &background);

    let outline = Circle::new(center, (diameter / 2.0 - 0.5).max(0.0));
    let outline_style = Stroke {
        width: 1.0,
        join: Join::Miter,
        miter_limit: 10.0,
        start_cap: Cap::Butt,
        end_cap: Cap::Butt,
        dash_pattern: Default::default(),
        dash_offset: 0.0,
    };
    let outline_color = checked.then_some(accent_color).unwrap_or(border_color);
    scene.stroke(&outline_style, transform, outline_color, None, &outline);

    if checked {
        let dot = Circle::new(center, (diameter * 0.3).max(0.0));
        scene.fill(Fill::NonZero, transform, accent_color, None, &dot);
    }
}
