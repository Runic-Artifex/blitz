//! CSS animation and transition events (`animationstart`, `animationiteration`,
//! `animationend`, `animationcancel`, `transitionrun`, `transitionstart`,
//! `transitionend`, `transitioncancel`) with Chromium 153's timing.
//!
//! Stylo's servo mode leaves events to the embedder and drops the state they
//! need (a canceled CSS animation and a finished transition leave the set
//! during the style traversal). The document therefore keeps a record of
//! every animation and transition it has seen, refreshed after every style
//! traversal, with the timing Chromium computes phases from (start time,
//! delay, duration, iteration count, a paused local time).
//!
//! Chromium samples animations once per rendering frame (its "update
//! animations" step), compares each one's phase and current iteration with
//! the previous sample, and queues the events its CSS animation and
//! transition event delegates derive from the change. A cancellation (a
//! style change, `display: none` on the element or an ancestor, removal) is
//! queued when the style update makes it, with the time of that update, and
//! dispatched with the next frame's events, before them. The embedder calls
//! [`BaseDocument::take_animation_events`] at each of its frames.
//!
//! Order within a frame (measured): queued cancellations in the order they
//! were made; then transitions before animations; elements in the order
//! Chromium compares their addresses (creation order in the measured cases;
//! node ids here); an element before its `::marker`, `::before` and
//! `::after`; transitions by generation then property name, animations by
//! their position in `animation-name`. Chromium orders the `transitionrun`
//! and `transitionstart` events of transitions an element starts in one style
//! change by an internal hash table: such events are marked
//! [`AnimationEvent::unordered`].

use crate::BaseDocument;
use blitz_traits::node_id::NodeId;
use style::Atom;
use style::animation::{AnimationSetKey, AnimationState, KeyframesIterationState};
use style::selector_parser::PseudoElement;

/// The kind of an animation or transition event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationEventKind {
    AnimationStart,
    AnimationIteration,
    AnimationEnd,
    AnimationCancel,
    TransitionRun,
    TransitionStart,
    TransitionEnd,
    TransitionCancel,
}

impl AnimationEventKind {
    /// `event.type`.
    pub fn name(self) -> &'static str {
        match self {
            Self::AnimationStart => "animationstart",
            Self::AnimationIteration => "animationiteration",
            Self::AnimationEnd => "animationend",
            Self::AnimationCancel => "animationcancel",
            Self::TransitionRun => "transitionrun",
            Self::TransitionStart => "transitionstart",
            Self::TransitionEnd => "transitionend",
            Self::TransitionCancel => "transitioncancel",
        }
    }
}

/// An event to dispatch at `node` (bubbling, as Chromium creates it).
#[derive(Clone, Debug, PartialEq)]
pub struct AnimationEvent {
    pub kind: AnimationEventKind,
    /// The owning element (the originating element of a pseudo-element).
    pub node: NodeId,
    /// `pseudoElement`: `""`, `"::before"`, `"::after"` or `"::marker"`.
    pub pseudo_element: &'static str,
    /// `animationName` (the `@keyframes` name) or `propertyName`.
    pub name: String,
    /// `elapsedTime`, in seconds.
    pub elapsed_time: f64,
    /// Chromium's order of this event relative to the one before it in the
    /// same batch is not known (transitions an element started in one
    /// style change, whose `transitionrun`/`transitionstart` Chromium orders
    /// by an internal hash table).
    pub unordered: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// Not yet sampled (Chromium's "idle").
    None,
    Before,
    Active,
    After,
}

#[derive(Clone, Debug, PartialEq)]
enum Ident {
    Animation(Atom),
    /// The property and the time the transition was created (bits).
    Transition(String, u64),
}

#[derive(Clone, Debug)]
struct Record {
    key: AnimationSetKey,
    ident: Ident,
    /// Chromium's start time: when it was created (before any delay).
    created: f64,
    delay: f64,
    duration: f64,
    /// The iteration count (infinite for `infinite`; 1 for transitions).
    iterations: f64,
    /// The local time of a paused animation.
    held: Option<f64>,
    /// The position of a CSS animation in its element's `animation-name`.
    index: usize,
    /// The phase and current iteration at the last sample.
    phase: Phase,
    iteration: Option<f64>,
    /// Seen in the current observation.
    seen: bool,
    /// A transition Stylo dropped because it ended before the next frame
    /// sampled it: kept for that sample.
    ended: bool,
}

impl Record {
    fn is_transition(&self) -> bool {
        matches!(self.ident, Ident::Transition(..))
    }

    fn local_time(&self, now: f64) -> f64 {
        self.held.unwrap_or(now - self.created)
    }

    fn active_duration(&self) -> f64 {
        if self.iterations.is_infinite() {
            f64::INFINITY
        } else {
            self.duration * self.iterations
        }
    }

    /// Chromium's phase and current iteration (`Timing::CalculatePhase`,
    /// `CurrentIteration`) at timeline time `now`, playing forwards.
    /// Times are compared in whole microseconds, as Chromium's
    /// `AnimationTimeDelta` holds them (Stylo's seconds would put a
    /// boundary reached exactly a rounding error before or after it).
    fn sample(&self, now: f64) -> (Phase, Option<f64>) {
        let us = |seconds: f64| (seconds * 1e6).round() as i64;
        let local = us(self.local_time(now));
        let delay = us(self.delay);
        let duration = us(self.duration);
        let active = if self.iterations.is_infinite() {
            i64::MAX / 4
        } else {
            us(self.duration * self.iterations)
        };
        let end = (delay + active).max(0);
        let before_active = delay.min(end).max(0);
        let active_after = (delay + active).min(end).max(0);
        if local < before_active {
            (Phase::Before, None)
        } else if local >= active_after {
            (Phase::After, None)
        } else {
            let iteration = if duration > 0 {
                (local - delay).div_euclid(duration) as f64
            } else {
                0.0
            };
            (Phase::Active, Some(iteration.max(0.0)))
        }
    }

    /// `elapsedTime` of a cancellation at `now`: an animation's current
    /// time, a transition's active time with fill mode `both`.
    fn cancel_time(&self, now: f64) -> f64 {
        let local = self.local_time(now);
        if self.is_transition() {
            (local - self.delay).clamp(0.0, self.duration)
        } else {
            local.max(0.0)
        }
    }

    fn interval_start(&self) -> f64 {
        (-self.delay).min(self.active_duration()).max(0.0)
    }

    fn interval_end(&self) -> f64 {
        let active = self.active_duration();
        let end = (self.delay + active).max(0.0);
        (end - self.delay).min(active).max(0.0)
    }

    fn name(&self) -> String {
        match &self.ident {
            Ident::Animation(name) => name.to_string(),
            Ident::Transition(property, _) => property.clone(),
        }
    }

    /// The events of the change from the last sample to (`phase`,
    /// `iteration`), as Chromium's `AnimationEventDelegate` and
    /// `TransitionEventDelegate` derive them (cancellations aside).
    fn events(&self, phase: Phase, iteration: Option<f64>) -> Vec<(AnimationEventKind, f64)> {
        use AnimationEventKind::*;
        use Phase::*;
        let prev = self.phase;
        let mut out = Vec::new();
        if self.is_transition() {
            if prev == phase {
                return out;
            }
            let from_delay = (-self.delay).max(0.0);
            if prev == None {
                out.push((TransitionRun, from_delay));
            }
            if matches!(phase, Active | After) && matches!(prev, None | Before) {
                out.push((TransitionStart, from_delay));
            } else if matches!(phase, Active | Before) && prev == After {
                out.push((TransitionStart, self.duration));
            }
            if phase == After && matches!(prev, Active | Before | None) {
                out.push((TransitionEnd, self.duration));
            } else if phase == Before && matches!(prev, Active | After) {
                out.push((TransitionEnd, from_delay));
            }
            return out;
        }
        let change = prev != phase;
        if change
            && ((matches!(prev, None | Before) && matches!(phase, Active | After))
                || (prev == After && matches!(phase, Active | Before)))
        {
            let elapsed = if prev == After {
                self.interval_end()
            } else {
                self.interval_start()
            };
            out.push((AnimationStart, elapsed));
        }
        if change && (phase == After || (matches!(prev, Active | After) && phase == Before)) {
            let elapsed = if phase == After {
                self.interval_end()
            } else {
                self.interval_start()
            };
            out.push((AnimationEnd, elapsed));
        }
        if !change && phase == Active && self.iteration != iteration {
            if let (Some(previous), Some(current)) = (self.iteration, iteration) {
                // One event for all iterations between two samples, with
                // the boundary of the current one (Chromium's
                // `IterationElapsedTime`).
                let boundary = if previous > current {
                    current + 1.0
                } else {
                    current
                };
                out.push((AnimationIteration, self.duration * boundary));
            }
        }
        out
    }

    /// Chromium's order of animations within a frame (see the module docs).
    fn order(&self) -> (u8, u64, u8, f64, String, usize) {
        let class = u8::from(!self.is_transition());
        let node = self.key.node.id() as u64;
        let pseudo = match &self.key.pseudo_element {
            None => 0,
            Some(PseudoElement::Marker) => 1,
            Some(PseudoElement::Before) => 2,
            Some(PseudoElement::After) => 3,
            Some(_) => 4,
        };
        match &self.ident {
            Ident::Transition(property, _) => {
                (class, node, pseudo, self.created, property.clone(), 0)
            }
            Ident::Animation(_) => (class, node, pseudo, 0.0, String::new(), self.index),
        }
    }
}

/// The document's animation records and queued cancellations.
#[derive(Default)]
pub(crate) struct AnimationEventTracker {
    records: Vec<Record>,
    queued: Vec<AnimationEvent>,
}

/// `elapsedTime` as Chromium computes it, from whole microseconds (Stylo
/// holds durations as single-precision seconds: 0.04 s is 0.03999999910593033).
fn whole_microseconds(seconds: f64) -> f64 {
    (seconds * 1e6).round() / 1e6
}

fn pseudo_name(key: &AnimationSetKey) -> &'static str {
    match &key.pseudo_element {
        None => "",
        Some(PseudoElement::Before) => "::before",
        Some(PseudoElement::After) => "::after",
        Some(PseudoElement::Marker) => "::marker",
        Some(_) => "",
    }
}

impl BaseDocument {
    /// Start (or stop) recording CSS animation and transition events, for
    /// an embedder that dispatches them ([`Self::take_animation_events`]).
    pub fn track_animation_events(&mut self, on: bool) {
        if on {
            self.animation_events.get_or_insert_with(Default::default);
        } else {
            self.animation_events = None;
        }
    }

    /// Whether a frame may have animation events to take: animations are
    /// recorded or cancellations queued.
    pub fn has_animation_events_pending(&self) -> bool {
        self.animation_events
            .as_ref()
            .is_some_and(|t| !t.records.is_empty() || !t.queued.is_empty())
    }

    /// A rendering frame at timeline time `now` (seconds, the time styles
    /// are resolved with): the cancellations queued since the last frame,
    /// then the events of each animation's change since the last frame, in
    /// Chromium's order.
    pub fn take_animation_events(&mut self, now: f64) -> Vec<AnimationEvent> {
        let Some(tracker) = self.animation_events.as_mut() else {
            return Vec::new();
        };
        let mut out = std::mem::take(&mut tracker.queued);
        let mut order: Vec<usize> = (0..tracker.records.len()).collect();
        order.sort_by(|&a, &b| {
            let (a, b) = (tracker.records[a].order(), tracker.records[b].order());
            a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal)
        });
        // Transitions an element started together, by the kind of event:
        // their relative order in Chromium is not known.
        let mut together: Vec<(AnimationSetKey, u64, AnimationEventKind, usize)> = Vec::new();
        for i in order {
            let record = &mut tracker.records[i];
            let (phase, iteration) = record.sample(now);
            for (kind, elapsed_time) in record.events(phase, iteration) {
                let mut unordered = false;
                if let Ident::Transition(_, created) = &record.ident {
                    if matches!(
                        kind,
                        AnimationEventKind::TransitionRun | AnimationEventKind::TransitionStart
                    ) {
                        for (key, c, k, at) in &together {
                            if *key == record.key && c == created && *k == kind {
                                unordered = true;
                                out[*at].unordered = true;
                            }
                        }
                        together.push((record.key.clone(), *created, kind, out.len()));
                    }
                }
                out.push(AnimationEvent {
                    kind,
                    node: NodeId::from_u64(record.key.node.id() as u64),
                    pseudo_element: pseudo_name(&record.key),
                    name: record.name(),
                    elapsed_time: whole_microseconds(elapsed_time),
                    unordered,
                });
            }
            record.phase = phase;
            record.iteration = iteration;
        }
        tracker.records.retain(|r| !r.ended);
        out
    }

    /// After a style traversal at timeline time `now`, before canceled
    /// animations are cleared: refresh the records from Stylo's animation
    /// sets, and queue the cancellations of the animations that are gone.
    pub(crate) fn observe_animation_events(&self, tracker: &mut AnimationEventTracker, now: f64) {
        for record in &mut tracker.records {
            record.seen = false;
        }
        {
            let sets = self.animations.sets.read();
            for (key, set) in sets.iter() {
                let node_id = NodeId::from_u64(key.node.id() as u64);
                let names: Vec<Atom> = self
                    .animation_names(node_id, key.pseudo_element.as_ref())
                    .unwrap_or_default();
                for animation in &set.animations {
                    if animation.state == AnimationState::Canceled {
                        continue;
                    }
                    let (current, iterations) = match animation.iteration_state {
                        KeyframesIterationState::Finite(current, max) => (current, max),
                        KeyframesIterationState::Infinite(current) => (current, f64::INFINITY),
                    };
                    // Stylo moves `started_at` by a duration per iteration.
                    let started = animation.started_at - animation.duration * current;
                    let held = match animation.state {
                        AnimationState::Paused(progress) => {
                            Some(animation.delay + (current + progress) * animation.duration)
                        }
                        _ => None,
                    };
                    let ident = Ident::Animation(animation.name.clone());
                    let index = names
                        .iter()
                        .position(|n| *n == animation.name)
                        .unwrap_or(usize::MAX);
                    let record = Record {
                        key: key.clone(),
                        ident,
                        created: started - animation.delay,
                        delay: animation.delay,
                        duration: animation.duration,
                        iterations,
                        held,
                        index,
                        phase: Phase::None,
                        iteration: None,
                        seen: true,
                        ended: false,
                    };
                    upsert(&mut tracker.records, record);
                }
                for transition in &set.transitions {
                    if transition.state == AnimationState::Canceled {
                        continue;
                    }
                    let created = transition.start_time - transition.delay;
                    let property = transition
                        .property_animation
                        .property_id()
                        .name()
                        .to_string();
                    let record = Record {
                        key: key.clone(),
                        ident: Ident::Transition(property, created.to_bits()),
                        created,
                        delay: transition.delay,
                        duration: transition.property_animation.duration,
                        iterations: 1.0,
                        held: None,
                        index: 0,
                        phase: Phase::None,
                        iteration: None,
                        seen: true,
                        ended: false,
                    };
                    upsert(&mut tracker.records, record);
                }
            }
        }
        // Gone: a transition Stylo dropped at its end (its element still in
        // the document) waits for the next frame's sample; anything else was
        // cancelled now.
        let mut cancelled: Vec<Record> = Vec::new();
        tracker.records.retain_mut(|record| {
            if record.seen || record.ended {
                return true;
            }
            let node_id = NodeId::from_u64(record.key.node.id() as u64);
            let in_document = self
                .nodes
                .get(node_id)
                .is_some_and(|n| n.flags.is_in_document());
            if record.is_transition() && in_document && record.sample(now).0 == Phase::After {
                record.ended = true;
                return true;
            }
            cancelled.push(record.clone());
            false
        });
        if !cancelled.is_empty() {
            // In the order the style update makes them: tree order, an
            // element's animations before its transitions.
            let order = self.document_order();
            cancelled.sort_by_key(|r| {
                let node = NodeId::from_u64(r.key.node.id() as u64);
                let position = order.get(&node).copied().unwrap_or(usize::MAX);
                (position, r.order().2, u8::from(r.is_transition()), r.index)
            });
            for record in cancelled {
                if matches!(record.phase, Phase::None | Phase::After) {
                    continue;
                }
                let kind = if record.is_transition() {
                    AnimationEventKind::TransitionCancel
                } else {
                    AnimationEventKind::AnimationCancel
                };
                tracker.queued.push(AnimationEvent {
                    kind,
                    node: NodeId::from_u64(record.key.node.id() as u64),
                    pseudo_element: pseudo_name(&record.key),
                    name: record.name(),
                    elapsed_time: whole_microseconds(record.cancel_time(now)),
                    unordered: false,
                });
            }
        }
    }

    /// The `animation-name` list of a node or one of its pseudo-elements.
    fn animation_names(&self, node: NodeId, pseudo: Option<&PseudoElement>) -> Option<Vec<Atom>> {
        let node = self.nodes.get(node)?;
        let data = node.try_stylo_element_data()?.get()?;
        let style = match pseudo {
            None => data.styles.get_primary()?.clone(),
            Some(p) => data.styles.pseudos.get(p)?.clone(),
        };
        Some(
            style
                .get_ui()
                .animation_name_iter()
                .filter_map(|n| n.as_atom().cloned())
                .collect(),
        )
    }

    /// Positions of the nodes in tree order (removed nodes are absent).
    fn document_order(&self) -> std::collections::HashMap<NodeId, usize> {
        let mut order = std::collections::HashMap::new();
        let mut stack = vec![self.root_node().id];
        while let Some(id) = stack.pop() {
            order.insert(id, order.len());
            if let Some(node) = self.nodes.get(id) {
                stack.extend(node.children.iter().rev().copied());
            }
        }
        order
    }

    /// Cancel the animations and transitions of elements that are not
    /// rendered because an ancestor is `display: none` (Stylo does not
    /// restyle such elements, so it kept them running; Chromium cancels
    /// them, and starts them again when the element is rendered again).
    pub(crate) fn cancel_undisplayed_animations(&self) {
        let mut sets = self.animations.sets.write();
        for (key, set) in sets.iter_mut() {
            let mut id = self
                .nodes
                .get(NodeId::from_u64(key.node.id() as u64))
                .and_then(|n| n.parent);
            while let Some(parent) = id {
                let Some(node) = self.nodes.get(parent) else {
                    break;
                };
                if node
                    .primary_styles()
                    .is_some_and(|s| s.clone_display().is_none())
                {
                    set.cancel_all_animations();
                    break;
                }
                id = node.parent;
            }
        }
    }
}

/// Insert `record`, or refresh the timing of the record with its identity
/// (keeping what the last sample saw).
fn upsert(records: &mut Vec<Record>, record: Record) {
    if let Some(existing) = records
        .iter_mut()
        .find(|r| !r.ended && r.key == record.key && r.ident == record.ident)
    {
        existing.created = record.created;
        existing.delay = record.delay;
        existing.duration = record.duration;
        existing.iterations = record.iterations;
        existing.held = record.held;
        existing.index = record.index;
        existing.seen = true;
    } else {
        records.push(record);
    }
}
