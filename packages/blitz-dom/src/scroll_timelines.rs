//! Scroll-driven CSS animations (`animation-timeline: scroll()`), with
//! Chromium 153's semantics.
//!
//! Stylo computes a scroll-driven animation's value from a timeline sample
//! (the scroller's offset in the timeline's axis and its maximum offset) held
//! on the animation. After layout, [`BaseDocument::resolve`] samples each
//! timeline from its scroller here; when a sample differs from the one the
//! last style update used, the document is restyled and laid out again with
//! the new samples. A scroll alone (wheel, programmatic) is picked up by the
//! sampling before the next style update. Scroll-driven animations do not
//! need animation ticks: a document whose only animations are scroll-driven
//! is idle while nothing scrolls or changes layout.
//!
//! What is not run (the animation has no effect) is reported by
//! [`BaseDocument::scroll_driven_animations`], for the embedder to report:
//! named and view timelines, named ranges, timings and play state changes
//! Stylo flags ([`style::animation::UnsupportedTimeline`]), and here
//! scrollers in a vertical writing mode, `scroll(nearest)` from an element
//! without a box, and pseudo-elements other than `::before` and `::after`.

use blitz_traits::node_id::NodeId;
use style::animation::{AnimationState, ScrollTimeline, ScrollTimelineSample, UnsupportedTimeline};
use style::invalidation::element::restyle_hints::RestyleHint;
use style::selector_parser::PseudoElement;
use style::values::computed::Overflow;
use style::values::specified::animation::{ScrollAxis, Scroller};

use crate::BaseDocument;
use crate::scrolling::ScrollTarget;

/// Why an animation that is not time-based has no effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnsupportedScrollDriven {
    /// What Stylo does not run: a named or view timeline, a named range, a
    /// timing that does not convert to the range, a play state change or a
    /// change of timeline.
    Timeline(UnsupportedTimeline),
    /// The scroller has a vertical writing mode (whose block axis and, for
    /// `vertical-rl`, scroll origin Blitz's scrolling does not represent).
    VerticalWritingMode,
    /// `scroll(nearest)` on an element that generates no box
    /// (`display: contents`).
    NoBox,
    /// An animation of a pseudo-element other than `::before` and `::after`.
    PseudoElement,
}

/// A CSS animation that is not on the document timeline (scroll-driven, or
/// with a timeline that is not supported), for an embedder that reports what
/// it cannot reproduce: such animations have no animation events here
/// (Chromium fires them by rules of its own), and the unsupported ones have
/// no effect.
#[derive(Clone, Debug, PartialEq)]
pub struct ScrollDrivenAnimation {
    /// The owning element (the originating element of a pseudo-element).
    pub node: NodeId,
    /// `""`, `"::before"`, `"::after"` or `"::marker"`.
    pub pseudo_element: &'static str,
    /// The `@keyframes` name.
    pub name: String,
    /// Why the animation has no effect, if it uses something unsupported.
    pub unsupported: Option<UnsupportedScrollDriven>,
    /// `animation-play-state: paused` (from the animation's start: it holds
    /// the start of its range. A later change of the play state is reported
    /// in `unsupported`).
    pub paused: bool,
}

fn pseudo_name(pseudo: Option<&PseudoElement>) -> &'static str {
    match pseudo {
        None => "",
        Some(PseudoElement::Before) => "::before",
        Some(PseudoElement::After) => "::after",
        Some(PseudoElement::Marker) => "::marker",
        Some(_) => "",
    }
}

impl BaseDocument {
    /// The CSS animations of the document that are not time-based: those on
    /// a scroll timeline (`animation-timeline: scroll()`) and those whose
    /// timeline (or range, timing or play state change) is not supported.
    /// Animations with `animation-timeline: none` from their start, which
    /// have neither an effect nor events in Chromium either, are not listed.
    pub fn scroll_driven_animations(&self) -> Vec<ScrollDrivenAnimation> {
        let sets = self.animations.sets.read();
        let mut out = Vec::new();
        for (key, set) in sets.iter() {
            let node = NodeId::from_u64(key.node.id() as u64);
            for animation in &set.animations {
                if animation.state == AnimationState::Canceled || animation.is_time_based() {
                    continue;
                }
                let unsupported = match animation.unsupported_timeline() {
                    Some(reason) => Some(UnsupportedScrollDriven::Timeline(reason)),
                    None => match animation.scroll_timeline() {
                        Some(timeline) => self
                            .sample_scroll_timeline(node, key.pseudo_element.as_ref(), timeline)
                            .err(),
                        // `animation-timeline: none`.
                        None => continue,
                    },
                };
                out.push(ScrollDrivenAnimation {
                    node,
                    pseudo_element: pseudo_name(key.pseudo_element.as_ref()),
                    name: animation.name.to_string(),
                    unsupported,
                    paused: matches!(animation.state, AnimationState::Paused(..)),
                });
            }
        }
        out.sort_by_key(|a| (a.node, a.pseudo_element, a.name.clone()));
        out
    }

    /// Sample every scroll timeline from the current layout and scroll
    /// offsets, store the samples on their animations and mark the elements
    /// whose samples changed for restyle. Returns whether any changed.
    pub(crate) fn update_scroll_timeline_samples(&mut self) -> bool {
        let sets = self.animations.sets.clone();
        let mut changed = Vec::new();
        {
            let mut sets = sets.write();
            for (key, set) in sets.iter_mut() {
                let node = NodeId::from_u64(key.node.id() as u64);
                for animation in set.animations.iter_mut() {
                    if animation.state == AnimationState::Canceled {
                        continue;
                    }
                    let Some(timeline) = animation.scroll_timeline() else {
                        continue;
                    };
                    let sample = self
                        .sample_scroll_timeline(node, key.pseudo_element.as_ref(), timeline)
                        .ok()
                        .flatten();
                    if animation.timeline_sample != sample {
                        animation.timeline_sample = sample;
                        changed.push(node);
                    }
                }
            }
        }
        for &node in &changed {
            if let Some(node) = self.nodes.get_mut(node) {
                if node.flags.is_in_document() {
                    node.set_restyle_hint(RestyleHint::RESTYLE_SELF);
                }
            }
        }
        !changed.is_empty()
    }

    /// The sample of a scroll timeline of an animation of `node` (or of its
    /// pseudo-element): `None` if the timeline is inactive (its scroller is
    /// not a scroll container or cannot scroll in the axis).
    fn sample_scroll_timeline(
        &self,
        node: NodeId,
        pseudo: Option<&PseudoElement>,
        timeline: &ScrollTimeline,
    ) -> Result<Option<ScrollTimelineSample>, UnsupportedScrollDriven> {
        let Some(element) = self.nodes.get(node) else {
            return Ok(None);
        };
        let subject = match pseudo {
            None => Some(node),
            Some(PseudoElement::Before) => element.before(),
            Some(PseudoElement::After) => element.after(),
            Some(_) => return Err(UnsupportedScrollDriven::PseudoElement),
        };
        let Some(subject) = subject else {
            return Ok(None);
        };
        let root = self.try_root_element().map(|root| root.id);
        let target = match timeline.scroller {
            Scroller::Root => ScrollTarget::Viewport,
            Scroller::SelfElement => {
                if Some(subject) == root {
                    ScrollTarget::Viewport
                } else if self.is_scroll_container(subject) {
                    ScrollTarget::Node(subject)
                } else {
                    return Ok(None);
                }
            }
            Scroller::Nearest => {
                let has_box = self.nodes[subject]
                    .primary_styles()
                    .is_some_and(|style| !style.clone_display().is_contents());
                if !has_box {
                    return Err(UnsupportedScrollDriven::NoBox);
                }
                self.nearest_scroll_container(subject)
            }
        };

        // The writing mode of the scroller (the viewport's is the root
        // element's).
        let styled = match target {
            ScrollTarget::Node(id) => Some(id),
            ScrollTarget::Viewport => root,
        };
        let vertical = styled
            .and_then(|id| self.nodes.get(id))
            .and_then(|node| node.primary_styles())
            .is_some_and(|style| style.writing_mode.is_vertical());
        if vertical {
            return Err(UnsupportedScrollDriven::VerticalWritingMode);
        }
        // Horizontal writing mode: block is y, inline is x.
        let horizontal = match timeline.axis {
            ScrollAxis::Inline | ScrollAxis::X => true,
            ScrollAxis::Block | ScrollAxis::Y => false,
        };

        let (offset, max) = self.scroll_state(target, true);
        let (offset, max) = if horizontal {
            // A right-to-left scroller scrolls leftwards from its origin at
            // the right: the timeline measures the distance from the origin.
            let offset = if self.scrolls_leftwards(target) {
                -offset.x
            } else {
                offset.x
            };
            (offset, max.x)
        } else {
            (offset.y, max.y)
        };
        if !(max > 0.0) {
            return Ok(None);
        }
        Ok(Some(ScrollTimelineSample { offset, max }))
    }

    /// Whether `node` is a scroll container: an element whose overflow is
    /// not `visible` or `clip` in either axis.
    fn is_scroll_container(&self, node: NodeId) -> bool {
        let scrolls = |overflow| {
            matches!(
                overflow,
                Overflow::Hidden | Overflow::Auto | Overflow::Scroll
            )
        };
        self.nodes.get(node).is_some_and(|node| {
            node.is_element()
                && node.primary_styles().is_some_and(|style| {
                    scrolls(style.clone_overflow_x()) || scrolls(style.clone_overflow_y())
                })
        })
    }

    /// The nearest scroll container that contains `node`'s box, along its
    /// containing blocks (as Chromium's `ContainingScrollContainer`), the
    /// element itself excluded; the viewport for the root element or when
    /// there is none.
    fn nearest_scroll_container(&self, node: NodeId) -> ScrollTarget {
        let root = self.try_root_element().map(|root| root.id);
        let mut current = self.nodes.get(node).and_then(|n| n.containing_block());
        while let Some(id) = current {
            if Some(id) == root {
                break;
            }
            if self.is_scroll_container(id) {
                return ScrollTarget::Node(id);
            }
            current = self.nodes.get(id).and_then(|n| n.containing_block());
        }
        ScrollTarget::Viewport
    }
}
