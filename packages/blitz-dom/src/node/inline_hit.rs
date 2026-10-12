//! Inline text and culled-parent hit order, separate from box/caret traversal.
use super::{Node, NodeData, TextLayout};
use crate::hit_geometry::HitLocation;
use blitz_traits::node_id::NodeId;
use kurbo::Rect;
use style::computed_values::{pointer_events::T as PointerEvents, visibility::T as Visibility};

struct Fragment {
    /// The actual text/atomic layout object, independent of its styled brush.
    object: NodeId,
    owner: NodeId,
    start: f32,
    end: f32,
    text_bounds: Option<Rect>,
}

impl TextLayout {
    pub(super) fn hit_inline(&self, root: &Node, location: HitLocation) -> Option<NodeId> {
        let scale = f64::from(self.layout.scale());
        let lines: Vec<_> = self.layout.lines().collect();
        for line in lines.into_iter().rev() {
            if !self.line_may_intersect(line, root, location) {
                continue;
            }
            // Own border and padding areas of inline boxes contain no descendant content;
            // deeper spans come later in style order.
            for (style_index, style) in self.layout.styles().iter().enumerate().rev() {
                let owner = style.brush.id;
                if owner == root.id || !eligible(root.with(owner)) {
                    continue;
                }
                let extents = line.inline_edge_extents(style_index);
                if extents.is_empty() {
                    continue;
                }
                let Some((top, bottom)) = self.span_block_bounds(line, root.with(owner), root)
                else {
                    continue;
                };
                for (x0, x1) in extents {
                    let rect = Rect::new(
                        f64::from(x0) / scale,
                        f64::from(top) / scale,
                        f64::from(x1) / scale,
                        f64::from(bottom) / scale,
                    );
                    // Snapped like the text fragments beside them.
                    if location.intersects_snapped_rect(rect) {
                        return Some(owner);
                    }
                }
            }
            let mut fragments: Vec<Fragment> = Vec::new();
            for item in line.items() {
                match item {
                    parley::PositionedLayoutItem::InlineBox(item)
                        if item.kind == parley::InlineBoxKind::InFlow =>
                    {
                        fragments.push(Fragment {
                            object: NodeId::from_u64(item.id),
                            owner: NodeId::from_u64(item.id),
                            start: item.x,
                            end: item.x + item.width,
                            text_bounds: None,
                        })
                    }
                    parley::PositionedLayoutItem::InlineBox(_) => {}
                    parley::PositionedLayoutItem::GlyphRun(run) => {
                        let owner = run.style().brush.id;
                        let Some((start, end)) =
                            self.run_inline_bounds(line, &run, root.with(owner))
                        else {
                            continue;
                        };
                        let Some((top, bottom)) =
                            line.style_content_bounds(usize::from(run.style_index()))
                        else {
                            continue;
                        };
                        let range = run.text_range();
                        let mut offset = run.offset();
                        let mut clusters = run
                            .run()
                            .visual_clusters()
                            .filter(|cluster| range.contains(&cluster.text_range().start))
                            .peekable();
                        while let Some(first) = clusters.next() {
                            let mut source_byte = first.text_range().start;
                            let mut advance = first.advance();
                            let mut continuation = first.is_ligature_continuation();
                            let (left_edge, mut right_edge) = first.inline_edge_insets();
                            while clusters.peek().is_some_and(|next| {
                                if run.run().is_rtl() {
                                    continuation
                                } else {
                                    next.is_ligature_continuation()
                                }
                            }) {
                                let cluster = clusters.next().unwrap();
                                source_byte = source_byte.min(cluster.text_range().start);
                                advance += cluster.advance();
                                continuation = cluster.is_ligature_continuation();
                                right_edge = cluster.inline_edge_insets().1;
                            }
                            let next = offset + advance;
                            let x0 = (offset + left_edge).max(start);
                            let x1 = (next - right_edge).min(end);
                            let object = self.text_source(source_byte).unwrap_or(owner);
                            if x0 < x1 {
                                // Keep shaping atoms intact, including multi-grapheme ligatures.
                                // Their logical first byte supplies ordering identity; the styled
                                // element remains the public pointer target.
                                let bounds = Rect::new(
                                    f64::from(x0) / scale,
                                    f64::from(top) / scale,
                                    f64::from(x1) / scale,
                                    f64::from(bottom) / scale,
                                );
                                if let Some(last) = fragments.last_mut().filter(|last| {
                                    last.object == object
                                        && last.owner == owner
                                        && last.end == x0
                                        && last.text_bounds.is_some()
                                }) {
                                    last.end = x1;
                                    last.text_bounds =
                                        Some(last.text_bounds.unwrap().union(bounds));
                                } else {
                                    fragments.push(Fragment {
                                        object,
                                        owner,
                                        start: x0,
                                        end: x1,
                                        text_bounds: Some(bounds),
                                    });
                                }
                            }
                            offset = next;
                        }
                    }
                }
            }
            for index in (0..fragments.len()).rev() {
                let fragment = &fragments[index];
                if eligible(root.with(fragment.owner))
                    && fragment
                        .text_bounds
                        .is_some_and(|rect| location.intersects_snapped_rect(rect))
                {
                    return Some(fragment.owner);
                }

                // A culled inline is tested only after its text/descendants have missed.
                // Earlier fragments of the same LayoutText are visited before its parents.
                let previous = index.checked_sub(1).map(|index| fragments[index].object);
                if previous == Some(fragment.object) {
                    continue;
                }
                let mut current = root.with(fragment.object);
                while let Some(parent) = current.parent {
                    if parent == root.id {
                        break;
                    }
                    let target = root.with(parent);
                    if has_layout_sibling(current)
                        && previous.is_some_and(|previous| in_subtree(root, previous, parent))
                    {
                        break;
                    }
                    if eligible(target) {
                        if let Some((top, bottom)) = self.span_block_bounds(line, target, root) {
                            // Keep line fragments separate. Inline ancestors use their own font
                            // box, while their inline extent includes descendant atomic content.
                            let mut bounds: Option<Rect> = None;
                            for child in &fragments {
                                if in_subtree(root, child.object, parent) {
                                    let rect = Rect::new(
                                        f64::from(child.start) / scale,
                                        f64::from(top) / scale,
                                        f64::from(child.end) / scale,
                                        f64::from(bottom) / scale,
                                    );
                                    bounds = Some(bounds.map_or(rect, |bounds| bounds.union(rect)));
                                } else {
                                    if bounds.is_some_and(|bounds| location.intersects_rect(bounds))
                                    {
                                        return Some(parent);
                                    }
                                    bounds = None;
                                }
                            }
                            if bounds.is_some_and(|bounds| location.intersects_rect(bounds)) {
                                return Some(parent);
                            }
                        }
                    }
                    current = target;
                }
            }
        }
        None
    }

    /// Conservative per-line traversal before snapped text targeting. Raw line
    /// boxes include leading; font/span boxes and glyph ink retain overflow.
    /// Unsupported ink effects escape culling rather than discarding a target.
    fn line_may_intersect(
        &self,
        line: parley::Line<'_, super::TextBrush>,
        root: &Node,
        location: HitLocation,
    ) -> bool {
        let scale = f64::from(self.layout.scale());
        let metrics = line.metrics();
        let rect = Rect::new(
            f64::from(metrics.inline_min_coord + metrics.offset) / scale,
            f64::from(metrics.block_min_coord) / scale,
            f64::from(metrics.inline_max_coord + metrics.offset) / scale,
            f64::from(metrics.block_max_coord) / scale,
        );
        if location.intersects_rect(rect) {
            return true;
        }
        for item in line.items() {
            let parley::PositionedLayoutItem::GlyphRun(run) = item else {
                // Atomic descendants have independent overflow/transform traversal.
                // Their culled parents can still be targeted outside the line.
                return true;
            };
            let mut node = root.with(run.style().brush.id);
            loop {
                if let Some((top, bottom)) = self.span_block_bounds(line, node, root) {
                    if location.intersects_rect(Rect::new(
                        f64::from(run.offset()) / scale,
                        f64::from(top) / scale,
                        f64::from(run.offset() + run.advance()) / scale,
                        f64::from(bottom) / scale,
                    )) {
                        return true;
                    }
                }
                if node.primary_styles().is_some_and(|style| {
                    !style.clone_text_shadow().0.is_empty()
                        || !style.clone_text_decoration_line().is_empty()
                }) {
                    return true;
                }
                if node.id == root.id {
                    break;
                }
                let Some(parent) = node.parent else { break };
                node = root.with(parent);
            }
            // Font-content boxes need not enclose tall marks or fallback glyph ink.
            // Reuse font data and variation coordinates without reshaping text.
            let shaped = run.run();
            if shaped.synthesis().skew().is_some() || shaped.synthesis().embolden() {
                return true;
            }
            use skrifa::MetadataProvider as _;
            let font = &shaped.font().font;
            let Ok(font_ref) = skrifa::FontRef::from_index(font.data.as_ref(), font.index) else {
                return true;
            };
            let coords: Vec<_> = shaped
                .normalized_coords()
                .iter()
                .map(|coord| skrifa::instance::NormalizedCoord::from_bits(coord.to_bits()))
                .collect();
            let glyph_metrics = font_ref.glyph_metrics(
                skrifa::instance::Size::new(shaped.font_size()),
                skrifa::instance::LocationRef::new(&coords),
            );
            for glyph in run.positioned_glyphs() {
                let Some(bounds) = glyph_metrics.bounds(skrifa::GlyphId::new(glyph.id)) else {
                    return true;
                };
                let rect = Rect::new(
                    f64::from(glyph.x + bounds.x_min) / scale,
                    f64::from(glyph.y - bounds.y_max) / scale,
                    f64::from(glyph.x + bounds.x_max) / scale,
                    f64::from(glyph.y - bounds.y_min) / scale,
                );
                if rect.width() > 0.
                    && rect.height() > 0.
                    && location.intersects_expanded_ink_rect(rect)
                {
                    return true;
                }
            }
        }
        false
    }
}

fn eligible(node: &Node) -> bool {
    !node.primary_styles().is_some_and(|style| {
        style.clone_pointer_events() == PointerEvents::None
            || matches!(
                style.clone_visibility(),
                Visibility::Hidden | Visibility::Collapse
            )
    })
}

fn in_subtree(root: &Node, mut node: NodeId, parent: NodeId) -> bool {
    loop {
        if node == parent {
            return true;
        }
        if node == root.id {
            return false;
        }
        let Some(next) = root.with(node).parent else {
            return false;
        };
        node = next;
    }
}

fn has_layout_sibling(node: &Node) -> bool {
    let Some(parent) = node.parent else {
        return false;
    };
    node.with(parent).children.iter().copied().any(|id| {
        if id == node.id {
            return false;
        }
        let sibling = node.with(id);
        match &sibling.data {
            NodeData::Text(text) => !text.content.is_empty(),
            NodeData::Element(_) | NodeData::AnonymousBlock(_) => sibling
                .primary_styles()
                .is_some_and(|style| !style.clone_display().is_none()),
            _ => false,
        }
    })
}
