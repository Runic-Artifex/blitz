//! Layout-time scroll anchoring for horizontal writing modes. This layer uses
//! existing DOM identity and visual geometry; table and flex layout need no
//! special anchoring rules.
use blitz_traits::node_id::NodeId;
use style::computed_values::position::T as Position;
use style::properties::ComputedValues;
use style::servo_arc::Arc;
use style::values::computed::{Overflow, OverflowAnchor};

use crate::{BaseDocument, BoundingRect, util::Point};

pub(crate) struct ScrollAnchor {
    scroller: NodeId,
    anchor: NodeId,
    offset: Point<f64>,
    relative_y: f64,
    path: Vec<(NodeId, Arc<ComputedValues>)>,
    fallbacks: Vec<AnchorFallback>,
}

struct AnchorFallback {
    anchor: NodeId,
    relative_y: f64,
    path: Vec<(NodeId, Arc<ComputedValues>)>,
}

impl BaseDocument {
    fn anchor_path(&self, anchor: NodeId, scroller: NodeId) -> Vec<(NodeId, Arc<ComputedValues>)> {
        let mut path = Vec::new();
        let mut next = Some(anchor);
        while let Some(id) = next {
            let Some(node) = self.nodes.get(id) else {
                break;
            };
            if let Some(style) = node.primary_styles() {
                path.push((id, Arc::clone(&style)));
            }
            if id == scroller {
                break;
            }
            next = node.parent;
        }
        path
    }

    fn live_anchor(&self, id: NodeId) -> bool {
        self.nodes.get(id).is_some_and(|n| {
            n.flags.is_in_document()
                && n.has_boxes()
                && n.primary_styles().is_some_and(|s| {
                    !s.clone_display().is_none()
                        && s.clone_overflow_anchor() != OverflowAnchor::None
                })
        })
    }
    fn anchor_port(&self, scroller: NodeId) -> Option<BoundingRect> {
        if self.try_root_element()?.id == scroller {
            let (width, height) = self.viewport.logical_size();
            return Some(BoundingRect {
                x: 0.0,
                y: 0.0,
                width: width as f64,
                height: height as f64,
            });
        }
        let node = self.nodes.get(scroller)?;
        let mut rect = self.get_client_bounding_rect(scroller)?;
        let border = node.unrounded_layout().border;
        rect.x += border.left as f64;
        rect.y += border.top as f64;
        rect.width = node.client_width() as f64;
        rect.height = node.client_height() as f64;
        Some(rect)
    }

    fn anchor_candidate(
        &self,
        id: NodeId,
        scroller: NodeId,
        port: &BoundingRect,
    ) -> Option<NodeId> {
        let node = self.nodes.get(id)?;
        let mut eligible = false;
        let mut fully_visible = false;
        if let Some(style) = node.primary_styles() {
            if style.clone_overflow_anchor() == OverflowAnchor::None
                || style.clone_display().is_none()
                || matches!(style.get_box().position, Position::Fixed | Position::Sticky)
            {
                return None;
            }
            // Out-of-flow descendants whose containing block bypasses the
            // scroller do not move with its content.
            if style.get_box().position == Position::Absolute {
                let mut cb = node.containing_block();
                let mut within = false;
                while let Some(parent) = cb {
                    if parent == scroller {
                        within = true;
                        break;
                    }
                    cb = self.nodes.get(parent).and_then(|n| n.containing_block());
                }
                if !within {
                    return None;
                }
            }
            if !node.is_anonymous() && node.has_boxes() {
                if let Some(mut rect) = self.get_client_bounding_rect(id) {
                    if !node.clips_overflow() {
                        rect.height = rect
                            .height
                            .max(node.unrounded_layout().scrollable_overflow_rect.bottom as f64);
                    }
                    eligible = rect.width > 0.0
                        && rect.height > 0.0
                        && rect.x < port.x + port.width
                        && rect.x + rect.width > port.x
                        && rect.y < port.y + port.height
                        && rect.y + rect.height > port.y;
                    fully_visible = eligible
                        && rect.x >= port.x
                        && rect.x + rect.width <= port.x + port.width
                        && rect.y >= port.y
                        && rect.y + rect.height <= port.y + port.height;
                    if fully_visible {
                        return Some(id);
                    }
                }
            }
        }
        for child in &node.children {
            if let Some(anchor) = self.anchor_candidate(*child, scroller, port) {
                return Some(anchor);
            }
        }
        (eligible && !fully_visible).then_some(id)
    }

    pub(crate) fn capture_scroll_anchors(&self) -> Vec<ScrollAnchor> {
        let Some(root) = self.try_root_element().map(|n| n.id) else {
            return Vec::new();
        };
        let mut anchors = Vec::new();
        for (id, node) in self.nodes.iter() {
            if node.element_data().is_none() {
                continue;
            }
            let offset = if id == root {
                self.viewport_scroll()
            } else {
                node.scroll_position()
            };
            if offset.y == 0.0 {
                continue;
            }
            let Some(style) = node.primary_styles() else {
                continue;
            };
            if node.is_anonymous()
                || !node.flags.is_in_document()
                || style.clone_overflow_anchor() == OverflowAnchor::None
                || !style.writing_mode.is_horizontal()
            {
                continue;
            }
            if id != root
                && !matches!(
                    style.clone_overflow_y(),
                    Overflow::Auto | Overflow::Scroll | Overflow::Hidden
                )
            {
                continue;
            }
            let Some(port) = self.anchor_port(id) else {
                continue;
            };
            let focused = self.focus_node_id.and_then(|focused| {
                let mut next = Some(focused);
                while let Some(parent) = next {
                    if parent == id {
                        return self.anchor_candidate(focused, id, &port);
                    }
                    let n = self.nodes.get(parent)?;
                    if n.primary_styles().is_some_and(|s| {
                        s.clone_overflow_anchor() == OverflowAnchor::None
                            || matches!(s.get_box().position, Position::Fixed | Position::Sticky)
                    }) {
                        return None;
                    }
                    next = n.parent;
                }
                None
            });
            let Some(anchor) = focused.or_else(|| {
                node.children
                    .iter()
                    .find_map(|child| self.anchor_candidate(*child, id, &port))
            }) else {
                continue;
            };
            let Some(rect) = self.get_client_bounding_rect(anchor) else {
                continue;
            };
            let path = self.anchor_path(anchor, id);
            // Retain the old visual positions of other visible subtrees. If
            // the chosen subtree disappears, a surviving sibling can anchor
            // the same viewport without consulting a stale removed box.
            let fallbacks = node
                .children
                .iter()
                .filter_map(|child| {
                    let candidate = self.anchor_candidate(*child, id, &port)?;
                    if candidate == anchor {
                        return None;
                    }
                    let rect = self.get_client_bounding_rect(candidate)?;
                    Some(AnchorFallback {
                        anchor: candidate,
                        relative_y: rect.y - port.y,
                        path: self.anchor_path(candidate, id),
                    })
                })
                .collect();
            anchors.push(ScrollAnchor {
                scroller: id,
                anchor,
                offset,
                relative_y: rect.y - port.y,
                path,
                fallbacks,
            });
        }
        // Inner scrollports settle first; their movement is then reflected in
        // the outer scrollport's candidate geometry.
        anchors.sort_by_key(|a| {
            let mut depth = 0;
            let mut next = self.nodes.get(a.scroller).and_then(|n| n.parent);
            while let Some(id) = next {
                depth += 1;
                next = self.nodes.get(id).and_then(|n| n.parent);
            }
            std::cmp::Reverse(depth)
        });
        anchors
    }

    pub(crate) fn apply_scroll_anchors(&mut self, anchors: Vec<ScrollAnchor>) {
        let root = self.try_root_element().map(|n| n.id);
        for mut anchor in anchors {
            // Identity alone is insufficient: mutation may have removed the
            // anchor or suppressed its box since the snapshot was captured.
            if !self.live_anchor(anchor.anchor) {
                let Some(fallback) = anchor
                    .fallbacks
                    .into_iter()
                    .find(|a| self.live_anchor(a.anchor))
                else {
                    continue;
                };
                anchor.anchor = fallback.anchor;
                anchor.relative_y = fallback.relative_y;
                anchor.path = fallback.path;
            }
            let Some(node) = self.nodes.get(anchor.scroller) else {
                continue;
            };
            let offset = if Some(anchor.scroller) == root {
                self.viewport_scroll()
            } else {
                node.scroll_position()
            };
            if offset != anchor.offset {
                continue;
            }
            let suppressed = anchor.path.iter().any(|(id, old)| {
                self.nodes
                    .get(*id)
                    .and_then(|n| n.primary_styles())
                    .is_none_or(|new| {
                        new.clone_overflow_anchor() == OverflowAnchor::None
                            || old.clone_display() != new.clone_display()
                            || old.get_box().position != new.get_box().position
                            || !old.top_equals(&new)
                            || !old.right_equals(&new)
                            || !old.bottom_equals(&new)
                            || !old.left_equals(&new)
                            || !old.width_equals(&new)
                            || !old.height_equals(&new)
                            || !old.min_width_equals(&new)
                            || !old.min_height_equals(&new)
                            || !old.max_width_equals(&new)
                            || !old.max_height_equals(&new)
                            || !old.margin_top_equals(&new)
                            || !old.margin_right_equals(&new)
                            || !old.margin_bottom_equals(&new)
                            || !old.margin_left_equals(&new)
                            || !old.padding_top_equals(&new)
                            || !old.padding_right_equals(&new)
                            || !old.padding_bottom_equals(&new)
                            || !old.padding_left_equals(&new)
                            || !old.transform_equals(&new)
                    })
            });
            if suppressed {
                continue;
            }
            let Some(port) = self.anchor_port(anchor.scroller) else {
                continue;
            };
            let Some(rect) = self.get_client_bounding_rect(anchor.anchor) else {
                continue;
            };
            let delta = (rect.y - port.y - anchor.relative_y).round();
            if delta != 0.0 {
                self.scroll_to_for_layout(
                    anchor.scroller,
                    Point {
                        x: offset.x,
                        y: offset.y + delta,
                    },
                );
            }
        }
    }
}
