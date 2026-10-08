//! Visual positioning layered over normal-flow layout. The painter, pointer
//! traversal and CSSOM all consume the same sticky offsets.
use style::computed_values::position::T as Position;
use style::values::computed::Overflow;
use style::values::specified::box_::DisplayOutside;
use taffy::{MaybeResolve, Point};

use crate::{Node, hit_geometry::HitLocation, layout::resolve_calc_value};

fn scroll_container(node: &Node) -> bool {
    !node.is_anonymous()
        && node.containing_block().is_some()
        && node.primary_styles().is_some_and(|s| {
            let scrolling = |o| matches!(o, Overflow::Auto | Overflow::Scroll | Overflow::Hidden);
            scrolling(s.clone_overflow_x()) || scrolling(s.clone_overflow_y())
        })
}

impl Node {
    /// CSS Backgrounds §4.5 uses one factor for overlapping corner radii.
    /// The padding clip then subtracts the adjacent border widths from each
    /// radius, matching the paths used by Blitz paint.
    pub(crate) fn rounded_box_intersects(
        &self,
        location: HitLocation,
        inset: taffy::Rect<f32>,
    ) -> bool {
        let size = self.final_layout().size;
        let (left, top, right, bottom) = (
            inset.left,
            inset.top,
            size.width - inset.right,
            size.height - inset.bottom,
        );
        let rect = kurbo::Rect::new(left as f64, top as f64, right as f64, bottom as f64);
        if !location.intersects_rect(rect) {
            return false;
        }
        let Some(styles) = self.primary_styles() else {
            return true;
        };
        // Native checkable widgets keep a rectangular clickable box even
        // when their painted checkbox/radio frame is smaller or circular.
        if self
            .layout_style()
            .flags
            .contains(stylo_taffy::StyleFlags::NATIVE_CHECKABLE_CONTROL)
        {
            return true;
        }
        let border = styles.get_border();
        let resolve = |radius: &style::values::computed::BorderCornerRadius| {
            (
                radius
                    .0
                    .width
                    .0
                    .resolve(style::values::computed::CSSPixelLength::new(size.width))
                    .px(),
                radius
                    .0
                    .height
                    .0
                    .resolve(style::values::computed::CSSPixelLength::new(size.height))
                    .px(),
            )
        };
        let mut radii = [
            resolve(&border.border_top_left_radius),
            resolve(&border.border_top_right_radius),
            resolve(&border.border_bottom_right_radius),
            resolve(&border.border_bottom_left_radius),
        ];
        let factor = 1.0f32
            .min(size.width / (radii[0].0 + radii[1].0))
            .min(size.width / (radii[3].0 + radii[2].0))
            .min(size.height / (radii[0].1 + radii[3].1))
            .min(size.height / (radii[1].1 + radii[2].1));
        for radius in &mut radii {
            radius.0 *= factor;
            radius.1 *= factor;
        }
        let radii = std::array::from_fn(|i| {
            let (rx, ry) = radii[i];
            let on_left = i == 0 || i == 3;
            let on_top = i < 2;
            let rx = (rx - if on_left { inset.left } else { inset.right }).max(0.0);
            let ry = (ry - if on_top { inset.top } else { inset.bottom }).max(0.0);
            (rx as f64, ry as f64)
        });
        location.intersects_rounded_rect(rect, radii)
    }

    pub(crate) fn hoisted_clips_contain(
        &self,
        child: &crate::HoistedPaintChild,
        location: HitLocation,
        scale: f64,
    ) -> bool {
        child
            .clip_ancestors(self.tree(), self.id)
            .into_iter()
            .all(|id| {
                let node = self.with(id);
                let position = crate::hoisted_child_position(self.tree(), self.id, id);
                let mut hit = location.translated(
                    -(position.x + node.visual_location().x) as f64,
                    -(position.y + node.visual_location().y) as f64,
                );
                if let Some(transform) = node.transform().as_deref() {
                    let Some(transformed) = hit.inverse_transform(*transform, scale) else {
                        return false;
                    };
                    hit = transformed;
                }
                node.rounded_box_intersects(hit, node.final_layout().border)
            })
    }

    /// Used location in the positioning parent's coordinates. Normal-flow
    /// sizing, offsets and overflow retain the original `Layout.location`.
    pub fn visual_location(&self) -> Point<f32> {
        let location = self.final_layout().location;
        let offset = self.sticky_offset();
        Point {
            x: location.x + offset.x,
            y: location.y + offset.y,
        }
    }

    pub(crate) fn sticky_offset(&self) -> Point<f32> {
        if self.is_anonymous()
            || !self
                .primary_styles()
                .is_some_and(|s| s.get_box().position == Position::Sticky)
        {
            return Point::ZERO;
        }
        let generation = self.tree().geometry_generation();
        let (stamp, cached) = self.sticky_offset_cache.get();
        if stamp == generation {
            return cached;
        }
        let offset = self.compute_sticky_offset();
        self.sticky_offset_cache.set((generation, offset));
        offset
    }

    fn compute_sticky_offset(&self) -> Point<f32> {
        let Some(parent) = self.containing_block().map(|id| self.with(id)) else {
            return Point::ZERO;
        };
        let layout = self.unrounded_layout();
        // This node's original border box, with ancestor sticky movement and
        // ancestor scrolling included, but without its own offset or scroll.
        let normal = parent.unrounded_absolute_position(layout.location.x, layout.location.y);
        let mut cb = parent;
        // Anonymous inline wrappers and table internal boxes do not limit a
        // sticky header to the height of a single line/row.
        while cb.is_anonymous()
            || cb
                .display_style()
                .is_some_and(|d| d.outside() == DisplayOutside::InternalTable)
        {
            let Some(next) = cb.containing_block() else {
                break;
            };
            cb = self.with(next);
        }
        let cb_layout = cb.unrounded_layout();
        let cb_scroll = cb.scroll_offset();
        let cb_origin = cb.unrounded_absolute_position(cb_scroll.x as f32, cb_scroll.y as f32);
        let mut cb_right = cb_layout.size.width - cb_layout.border.right - cb_layout.padding.right;
        let mut cb_bottom =
            cb_layout.size.height - cb_layout.border.bottom - cb_layout.padding.bottom;
        if scroll_container(cb) {
            cb_right =
                cb_right.max(cb_layout.scrollable_overflow_rect.right - cb_layout.padding.right);
            cb_bottom =
                cb_bottom.max(cb_layout.scrollable_overflow_rect.bottom - cb_layout.padding.bottom);
        }
        let cb_left =
            cb_origin.x + cb_layout.border.left + cb_layout.padding.left - cb_scroll.x as f32;
        let cb_top =
            cb_origin.y + cb_layout.border.top + cb_layout.padding.top - cb_scroll.y as f32;
        let cb_right = cb_origin.x + cb_right - cb_scroll.x as f32;
        let cb_bottom = cb_origin.y + cb_bottom - cb_scroll.y as f32;

        let mut ancestor = Some(parent);
        let mut scroller = None;
        while let Some(node) = ancestor {
            if scroll_container(node) {
                scroller = Some(node);
                break;
            }
            ancestor = node.containing_block().map(|id| self.with(id));
        }
        let (port_x, port_y, width, height) = match scroller {
            Some(node) => {
                let own_scroll = node.scroll_offset();
                let origin =
                    node.unrounded_absolute_position(own_scroll.x as f32, own_scroll.y as f32);
                let border = node.unrounded_layout().border;
                (
                    origin.x + border.left,
                    origin.y + border.top,
                    node.client_width(),
                    node.client_height(),
                )
            }
            None => {
                let origin = self.tree().viewport_scroll();
                let size = self.tree().viewport_size();
                (origin.x as f32, origin.y as f32, size.width, size.height)
            }
        };
        let styles = self.primary_styles().unwrap();
        let inset = stylo_taffy::convert::inset_rect(&styles);
        let left = inset.left.maybe_resolve(Some(width), resolve_calc_value);
        let right = inset.right.maybe_resolve(Some(width), resolve_calc_value);
        let top = inset.top.maybe_resolve(Some(height), resolve_calc_value);
        let bottom = inset.bottom.maybe_resolve(Some(height), resolve_calc_value);
        let rtl = styles.clone_direction() == style::computed_values::direction::T::Rtl;
        Point {
            x: sticky_axis(
                normal.x,
                layout.size.width,
                port_x,
                width,
                cb_left,
                cb_right,
                left,
                right,
                layout.margin.left,
                layout.margin.right,
                rtl,
            ),
            y: sticky_axis(
                normal.y,
                layout.size.height,
                port_y,
                height,
                cb_top,
                cb_bottom,
                top,
                bottom,
                layout.margin.top,
                layout.margin.bottom,
                false,
            ),
        }
    }
}

/// CSS Positioned Layout §3.4: constrain the border box to the sticky view
/// rectangle and its position box to the containing block. An automatic inset
/// participates in view-rectangle sizing, but never pushes that side.
#[allow(clippy::too_many_arguments)]
fn sticky_axis(
    normal: f32,
    size: f32,
    port: f32,
    port_size: f32,
    cb_start: f32,
    cb_end: f32,
    start: Option<f32>,
    end: Option<f32>,
    margin_start: f32,
    margin_end: f32,
    flipped: bool,
) -> f32 {
    if start.is_none() && end.is_none() {
        return 0.0;
    }
    let mut low = port + start.unwrap_or(0.0);
    let mut high = port + port_size - end.unwrap_or(0.0);
    if high - low < size {
        if flipped {
            low = high - size;
        } else {
            high = low + size;
        }
    }
    let min_shift = (cb_start + margin_start.min((normal - cb_start).max(0.0)) - normal).min(0.0);
    let max_shift =
        (cb_end - margin_end.min((cb_end - normal - size).max(0.0)) - normal - size).max(0.0);
    let mut shift: f32 = 0.0;
    let push_start = |shift: f32| {
        if start.is_some() {
            shift.max((low - normal).min(max_shift))
        } else {
            shift
        }
    };
    let push_end = |shift: f32| {
        if end.is_some() {
            shift.min((high - normal - size).max(min_shift))
        } else {
            shift
        }
    };
    if flipped {
        shift = push_end(push_start(shift));
    } else {
        shift = push_start(push_end(shift));
    }
    shift
}
