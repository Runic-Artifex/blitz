//! Pointer geometry in CSS coordinates, independent of caret and DOM rectangles.
use kurbo::{Affine, Point, Rect, Vec2};

/// A point and its one-CSS-pixel footprint. Construct once at traversal entry;
/// translations and inverse transforms move the footprint along with the point.
#[derive(Clone, Copy, Debug)]
pub(crate) struct HitLocation {
    point: Point,
    quad: [Point; 4],
    bounds: Rect,
    rectilinear: bool,
    // Accumulated offset in the current paint layer. Text snapping is applied
    // after this offset, rather than independently at each fragment's origin.
    paint_offset: Vec2,
}

impl HitLocation {
    pub(crate) fn new(x: f32, y: f32) -> Self {
        let point = Point::new(f64::from(x), f64::from(y));
        let x = (point.x * 64.0).floor() / 64.0;
        let y = (point.y * 64.0).floor() / 64.0;
        let bounds = Rect::new(x, y, x + 1.0, y + 1.0);
        Self {
            point,
            quad: rect_points(bounds),
            bounds,
            rectilinear: true,
            paint_offset: Vec2::ZERO,
        }
    }

    pub(crate) fn point(self) -> Point {
        self.point
    }

    pub(crate) fn translated(mut self, x: f64, y: f64) -> Self {
        let delta = Vec2::new(x, y);
        self.point += delta;
        for point in &mut self.quad {
            *point += delta;
        }
        self.bounds = self.bounds + delta;
        self.paint_offset -= delta;
        self
    }

    /// Node transforms are stored in device coordinates; the query stays in CSS
    /// coordinates. A singular/nonfinite transform cannot produce a hit.
    pub(crate) fn inverse_transform(self, transform: Affine, scale: f64) -> Option<Self> {
        let determinant = transform.determinant();
        if !determinant.is_finite() || determinant == 0.0 || !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        let (correction, residual) = layer_phase(self.paint_offset, transform);
        let location = self.translated(correction.x, correction.y);
        let inverse = Affine::scale(1.0 / scale) * transform.inverse() * Affine::scale(scale);
        let point = inverse * location.point;
        let quad = location.quad.map(|p| inverse * p);
        if !point.x.is_finite()
            || !point.y.is_finite()
            || quad.iter().any(|p| !p.x.is_finite() || !p.y.is_finite())
        {
            return None;
        }
        Some(Self {
            point,
            quad,
            bounds: quad_bounds(quad),
            rectilinear: (0..4).all(|i| {
                let edge = quad[(i + 1) % 4] - quad[i];
                edge.x == 0.0 || edge.y == 0.0
            }),
            paint_offset: residual,
        })
    }

    /// Positive-area intersection, including an exact quad test for rotation or
    /// skew. The bounding rectangle alone must not turn a near miss into a hit.
    pub(crate) fn intersects_rect(self, rect: Rect) -> bool {
        if !positive_rect(rect)
            || !positive_rect(self.bounds)
            || self.bounds.x1 <= rect.x0
            || self.bounds.x0 >= rect.x1
            || self.bounds.y1 <= rect.y0
            || self.bounds.y0 >= rect.y1
        {
            return false;
        }
        if self.rectilinear {
            return true;
        }
        let corners = rect_points(rect);
        for i in 0..4 {
            let edge = self.quad[(i + 1) % 4] - self.quad[i];
            let axis = Vec2::new(-edge.y, edge.x);
            let (a0, a1) = project(self.quad, axis);
            let (b0, b1) = project(corners, axis);
            if a1 <= b0 || b1 <= a0 {
                return false;
            }
        }
        true
    }

    /// Text and decorated inline boxes use snapped hit rectangles; culled
    /// ancestors, ordinary blocks and traversal/overflow bounds use raw ones.
    pub(crate) fn intersects_snapped_rect(self, rect: Rect) -> bool {
        self.intersects_rect(self.snapped_rect(rect))
    }

    pub(crate) fn snapped_rect(self, rect: Rect) -> Rect {
        let offset = self.paint_offset;
        // LayoutUnit::Round uses positive-infinity half ties, including -0.5.
        let round = |value: f64| (value + 0.5).floor();
        Rect::new(
            round(rect.x0 + offset.x) - offset.x,
            round(rect.y0 + offset.y) - offset.y,
            round(rect.x1 + offset.x) - offset.x,
            round(rect.y1 + offset.y) - offset.y,
        )
    }

    pub(crate) fn intersects_rounded_rect(self, rect: Rect, radii: [(f64, f64); 4]) -> bool {
        if !self.intersects_rect(rect) {
            return false;
        }
        for (i, (rx, ry)) in radii.into_iter().enumerate() {
            if rx <= 0.0 || ry <= 0.0 {
                continue;
            }
            let left = i == 0 || i == 3;
            let top = i < 2;
            let cx = if left { rect.x0 + rx } else { rect.x1 - rx };
            let cy = if top { rect.y0 + ry } else { rect.y1 - ry };
            let corner = Rect::new(
                if left { rect.x0 } else { cx },
                if top { rect.y0 } else { cy },
                if left { cx } else { rect.x1 },
                if top { cy } else { rect.y1 },
            );
            if self.intersects_rect(corner) && !self.intersects_ellipse(Point::new(cx, cy), rx, ry)
            {
                return false;
            }
        }
        true
    }

    fn intersects_ellipse(self, center: Point, rx: f64, ry: f64) -> bool {
        let quad = self
            .quad
            .map(|p| Vec2::new((p.x - center.x) / rx, (p.y - center.y) / ry));
        if quad.iter().any(|p| p.length_squared() <= 1.0) {
            return true;
        }
        let mut positive = false;
        let mut negative = false;
        for i in 0..4 {
            let a = quad[i];
            let b = quad[(i + 1) % 4];
            let edge = b - a;
            let cross = edge.cross(-a);
            positive |= cross > 0.0;
            negative |= cross < 0.0;
            let t = (-a.dot(edge) / edge.length_squared()).clamp(0.0, 1.0);
            if (a + edge * t).length_squared() <= 1.0 {
                return true;
            }
        }
        !(positive && negative) // The ellipse center lies inside the quad.
    }
}

impl crate::Node {
    /// Conservative bounds for hoisted candidates in the same logical position
    /// space as CSSOM and raw pointer boxes; the painter's cache stays separate.
    pub(crate) fn hoisted_hit_bounds(&self, location: HitLocation, scale: f64) -> Option<Rect> {
        let context = self.stacking_context.as_ref()?;
        let mut bounds: Option<Rect> = None;
        for child in context
            .neg_z_hoisted_children()
            .chain(context.pos_z_hoisted_children())
        {
            let node = self.with(child.node_id);
            let position = crate::layout::paint_tree::logical_hoisted_child_position(
                self.tree(),
                self.id,
                child.node_id,
            );
            let location_for_node = node.logical_visual_location();
            let overflow = *node.scrollable_overflow();
            let size = node.unrounded_layout().size;
            let mut rect = Rect::new(
                overflow.x0 / scale,
                overflow.y0 / scale,
                overflow.x1 / scale,
                overflow.y1 / scale,
            )
            .union(Rect::new(0.0, 0.0, size.width as f64, size.height as f64));
            let mut offset = Vec2::new(
                (position.x + location_for_node.x) as f64,
                (position.y + location_for_node.y) as f64,
            );
            if let Some(transform) = node.transform().as_deref() {
                rect = (Affine::scale(1.0 / scale) * *transform * Affine::scale(scale))
                    .transform_rect_bbox(rect);
                offset -= layer_phase(location.paint_offset + offset, *transform).0;
            }
            rect = rect + offset;
            bounds = Some(bounds.map_or(rect, |bounds| bounds.union(rect)));
        }
        bounds
    }
}

/// Paint-offset translations round once at a transform layer boundary. Only
/// translation and unit-scale axes carry their remaining subpixel accumulation.
fn layer_phase(offset: Vec2, transform: Affine) -> (Vec2, Vec2) {
    let [a, b, c, d, _, _] = transform.as_coeffs();
    let residual = Vec2::new(
        offset.x - (offset.x + 0.5).floor(),
        offset.y - (offset.y + 0.5).floor(),
    );
    let diagonal = b == 0.0 && c == 0.0;
    let propagated = Vec2::new(
        if diagonal && a == 1.0 {
            residual.x
        } else {
            0.0
        },
        if diagonal && d == 1.0 {
            residual.y
        } else {
            0.0
        },
    );
    (residual - propagated, propagated)
}

fn positive_rect(rect: Rect) -> bool {
    rect.x0.is_finite()
        && rect.y0.is_finite()
        && rect.x1.is_finite()
        && rect.y1.is_finite()
        && rect.x1 > rect.x0
        && rect.y1 > rect.y0
}
fn rect_points(rect: Rect) -> [Point; 4] {
    [
        Point::new(rect.x0, rect.y0),
        Point::new(rect.x1, rect.y0),
        Point::new(rect.x1, rect.y1),
        Point::new(rect.x0, rect.y1),
    ]
}
fn quad_bounds(quad: [Point; 4]) -> Rect {
    Rect::new(
        quad.iter().map(|p| p.x).fold(f64::INFINITY, f64::min),
        quad.iter().map(|p| p.y).fold(f64::INFINITY, f64::min),
        quad.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max),
        quad.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max),
    )
}
fn project(points: [Point; 4], axis: Vec2) -> (f64, f64) {
    points
        .into_iter()
        .map(|p| p.to_vec2().dot(axis))
        .fold((f64::INFINITY, f64::NEG_INFINITY), |(min, max), p| {
            (min.min(p), max.max(p))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_edges_empty_bounds_and_layout_unit_floor() {
        let rect = Rect::new(20.0, 20.0, 40.0, 40.0);
        assert!(!HitLocation::new(19.0, 30.0).intersects_rect(rect));
        assert!(HitLocation::new(19.015625, 30.0).intersects_rect(rect));
        assert!(!HitLocation::new(19.01, 30.0).intersects_rect(rect));
        assert!(!HitLocation::new(40.0, 30.0).intersects_rect(rect));
        assert!(!HitLocation::new(20.0, 20.0).intersects_rect(Rect::ZERO));
        assert!(!HitLocation::new(f32::NAN, 20.0).intersects_rect(rect));
    }

    #[test]
    fn text_snaps_after_accumulated_offset_with_negative_half_ties() {
        let hit = HitLocation::new(30.0, 50.375).translated(-20.375, -20.375);
        let fragment = Rect::new(0.0, 31.0, 68.25, 59.0);
        assert!(!hit.intersects_rect(fragment));
        assert!(hit.intersects_snapped_rect(fragment));
        assert_eq!(
            HitLocation::new(0.0, 0.0).snapped_rect(Rect::new(-0.5, -0.515625, 2.5, 3.5)),
            Rect::new(0.0, -1.0, 3.0, 4.0)
        );
        assert!(
            !HitLocation::new(30.0, 68.375)
                .translated(-20.375, -20.375)
                .intersects_snapped_rect(Rect::new(0.0, 58.0, 68.25, 86.0))
        );
    }

    #[test]
    fn inverse_scale_preserves_the_original_footprint() {
        let hit = HitLocation::new(19.5, 25.0)
            .inverse_transform(Affine::scale(2.0), 1.0)
            .unwrap();
        assert_eq!(hit.bounds.width(), 0.5);
        assert!(hit.intersects_rect(Rect::new(10.0, 10.0, 20.0, 20.0)));
        let hit = HitLocation::new(9.75, 7.5)
            .inverse_transform(Affine::scale(0.5), 2.0)
            .unwrap();
        assert_eq!(hit.bounds.width(), 2.0);
        assert!(hit.intersects_rect(Rect::new(20.0, 10.0, 40.0, 20.0)));
        assert!(
            HitLocation::new(1.0, 1.0)
                .inverse_transform(Affine::scale(0.0), 1.0)
                .is_none()
        );
    }

    #[test]
    fn transform_layers_preserve_subpixels_only_on_unit_scale_axes() {
        let query = HitLocation::new(10.75, 20.75).translated(-10.375, -20.375);
        let identity = query.inverse_transform(Affine::IDENTITY, 1.0).unwrap();
        assert_eq!(identity.point(), Point::new(0.375, 0.375));
        assert_eq!(identity.paint_offset, Vec2::new(0.375, 0.375));
        let mixed = query
            .inverse_transform(Affine::scale_non_uniform(1.0, 0.5), 1.0)
            .unwrap();
        assert_eq!(mixed.point(), Point::new(0.375, 1.5));
        assert_eq!(mixed.paint_offset, Vec2::new(0.375, 0.0));
        assert_eq!(mixed.bounds.size(), kurbo::Size::new(1.0, 2.0));
        let negative = HitLocation::new(0.0, 0.0)
            .translated(0.5, 0.5)
            .inverse_transform(Affine::scale(0.5), 1.0)
            .unwrap();
        assert_eq!(negative.point(), Point::ZERO);
        assert_eq!(negative.paint_offset, Vec2::ZERO);
    }

    #[test]
    fn rotated_quad_rejects_a_bounding_box_only_contact() {
        let hit = HitLocation::new(10.0, 10.0)
            .inverse_transform(Affine::rotate(std::f64::consts::FRAC_PI_4), 1.0)
            .unwrap();
        assert!(!hit.intersects_rect(Rect::new(14.15, 0.55, 14.25, 0.65)));
        assert!(hit.intersects_rect(Rect::new(14.7, -0.05, 14.8, 0.05)));
    }

    #[test]
    fn rounded_corner_checks_the_quad_against_the_ellipse() {
        let rect = Rect::new(0.0, 0.0, 40.0, 30.0);
        let radii = [(12.0, 12.0); 4];
        assert!(!HitLocation::new(1.0, 1.0).intersects_rounded_rect(rect, radii));
        assert!(HitLocation::new(4.0, 4.0).intersects_rounded_rect(rect, radii));
        assert!(HitLocation::new(-0.5, 14.0).intersects_rounded_rect(rect, radii));
    }
}
