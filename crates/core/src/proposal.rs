//! Systems PROPOSE; the engine applies or rejects.
use crate::component::{AnyComponent, Component};
use crate::event::{Handle, Payload, Schedule};
use crate::id::{EdgeId, EntityId, EventId, SystemId};
use crate::relax::{Reach, Relax};
use crate::time::{SimTime, Span};
use crate::value::Value;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub enum EdgeOp {
    /// src is the proposal's entity
    Open { kind: &'static str, dst: EntityId, strength: Option<Relax> },
    Close { id: EdgeId },
    /// close `id`, open a successor with a new strength anchor
    Reanchor { id: EdgeId, strength: Relax },
}

#[derive(Debug)]
pub enum Change {
    /// Replace the whole component; the engine diffs it field by field.
    Set(Box<dyn AnyComponent>),
    Insert(Box<dyn AnyComponent>),
    Remove { kind: &'static str },
    Edge(EdgeOp),
    /// The entity must have been reserved by this handler via `Ctx::reserve`.
    Spawn { components: Vec<Box<dyn AnyComponent>> },
    Despawn,
}

#[derive(Debug)]
pub struct Proposal { pub entity: EntityId, pub change: Change, pub by: SystemId, pub cause: EventId }

/// rule, entity, component, field, old, attempted, reason. Always.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Violation {
    pub rule: String,
    pub entity: EntityId,
    pub component: Option<String>,
    pub field: Option<String>,
    pub old: Value,
    pub attempted: Value,
    pub reason: String,
}
impl std::fmt::Display for Violation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {} {}.{}: {} → {}: {}", self.rule, self.entity,
            self.component.as_deref().unwrap_or("-"), self.field.as_deref().unwrap_or("-"), self.old, self.attempted, self.reason)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Verdict { Applied, Noop, Rejected(Violation) }

#[derive(Clone, Debug, PartialEq)]
pub enum Note {
    /// A crossing was asked for that the trajectory can never reach. Logged, counted, never scheduled.
    Unreachable { why: &'static str, threshold: f64, x_inf: f64 },
    Info(String),
}

/// What a handler returns. `by` and `cause` are stamped by the engine.
#[derive(Default, Debug)]
pub struct Out {
    pub changes: Vec<(EntityId, Change)>,
    pub schedules: Vec<Schedule>,
    pub cancels: Vec<Handle>,
    /// cancel every pending event of this kind for this subject (the reschedule pattern)
    pub cancels_for: Vec<(EntityId, &'static str)>,
    /// cancel only this plugin's own pending crossings: (subject, kind, why)
    pub cancels_for_why: Vec<(EntityId, &'static str, &'static str)>,
    pub notes: Vec<Note>,
}

impl Out {
    pub fn new() -> Out { Out::default() }
    pub fn set<T: Component>(&mut self, e: EntityId, c: T) -> &mut Self { self.changes.push((e, Change::Set(Box::new(c)))); self }
    pub fn set_any(&mut self, e: EntityId, c: Box<dyn AnyComponent>) -> &mut Self { self.changes.push((e, Change::Set(c))); self }
    pub fn insert<T: Component>(&mut self, e: EntityId, c: T) -> &mut Self { self.changes.push((e, Change::Insert(Box::new(c)))); self }
    pub fn insert_any(&mut self, e: EntityId, c: Box<dyn AnyComponent>) -> &mut Self { self.changes.push((e, Change::Insert(c))); self }
    pub fn remove<T: Component>(&mut self, e: EntityId) -> &mut Self { self.changes.push((e, Change::Remove { kind: T::NAME })); self }
    pub fn remove_named(&mut self, e: EntityId, kind: &'static str) -> &mut Self { self.changes.push((e, Change::Remove { kind })); self }
    pub fn spawn(&mut self, e: EntityId, components: Vec<Box<dyn AnyComponent>>) -> &mut Self { self.changes.push((e, Change::Spawn { components })); self }
    pub fn despawn(&mut self, e: EntityId) -> &mut Self { self.changes.push((e, Change::Despawn)); self }
    pub fn edge_open(&mut self, src: EntityId, kind: &'static str, dst: EntityId, strength: Option<Relax>) -> &mut Self {
        self.changes.push((src, Change::Edge(EdgeOp::Open { kind, dst, strength }))); self
    }
    pub fn edge_close(&mut self, src: EntityId, id: EdgeId) -> &mut Self { self.changes.push((src, Change::Edge(EdgeOp::Close { id }))); self }
    pub fn edge_reanchor(&mut self, src: EntityId, id: EdgeId, strength: Relax) -> &mut Self { self.changes.push((src, Change::Edge(EdgeOp::Reanchor { id, strength }))); self }
    pub fn schedule(&mut self, s: Schedule) -> &mut Self { self.schedules.push(s); self }
    pub fn schedule_at(&mut self, at: SimTime, kind: &'static str, subject: EntityId, why: &'static str) -> &mut Self {
        self.schedules.push(Schedule::new(at, kind, subject, why)); self
    }
    pub fn schedule_in(&mut self, now: SimTime, delay: Span, kind: &'static str, subject: EntityId, why: &'static str) -> &mut Self {
        self.schedules.push(Schedule::new(now + delay, kind, subject, why)); self
    }
    pub fn schedule_with<T: Serialize>(&mut self, at: SimTime, kind: &'static str, subject: EntityId, why: &'static str, payload: &T) -> &mut Self {
        self.schedules.push(Schedule::new(at, kind, subject, why).with_payload(Payload::of(payload))); self
    }
    /// Schedule at a computed crossing. `Never` is refused and noted (the sleep-bar bug, caught here).
    /// `Already` schedules immediately. Returns whether anything was scheduled.
    pub fn schedule_reach(&mut self, now: SimTime, reach: Reach, threshold: f64, kind: &'static str, subject: EntityId, why: &'static str) -> bool {
        match reach {
            Reach::At(t) => { self.schedules.push(Schedule::new(t.max(now), kind, subject, why)); true }
            Reach::Already => { self.schedules.push(Schedule::new(now, kind, subject, why)); true }
            Reach::Never { x_inf } => { self.notes.push(Note::Unreachable { why, threshold, x_inf }); false }
        }
    }
    /// `schedule_reach` with a payload.
    #[allow(clippy::too_many_arguments)]
    pub fn schedule_reach_with<T: Serialize>(&mut self, now: SimTime, reach: Reach, threshold: f64, kind: &'static str, subject: EntityId, why: &'static str, payload: &T) -> bool {
        match reach {
            Reach::At(t) => { self.schedules.push(Schedule::new(t.max(now), kind, subject, why).with_payload(Payload::of(payload))); true }
            Reach::Already => { self.schedules.push(Schedule::new(now, kind, subject, why).with_payload(Payload::of(payload))); true }
            Reach::Never { x_inf } => { self.notes.push(Note::Unreachable { why, threshold, x_inf }); false }
        }
    }
    /// Schedule at a crossing IF it will happen; an unreachable crossing is simply not scheduled
    /// (a question, not a requirement - thermostats, band edges). Returns whether it was.
    pub fn schedule_if_reachable_with<T: Serialize>(&mut self, now: SimTime, reach: Reach, kind: &'static str, subject: EntityId, why: &'static str, payload: &T) -> bool {
        match reach {
            Reach::At(t) => { self.schedules.push(Schedule::new(t.max(now), kind, subject, why).with_payload(Payload::of(payload))); true }
            Reach::Already => { self.schedules.push(Schedule::new(now, kind, subject, why).with_payload(Payload::of(payload))); true }
            Reach::Never { .. } => false,
        }
    }
    pub fn cancel(&mut self, h: Handle) -> &mut Self { self.cancels.push(h); self }
    pub fn cancel_pending(&mut self, subject: EntityId, kind: &'static str) -> &mut Self { self.cancels_for.push((subject, kind)); self }
    /// Cancel pending events of `kind` for `subject` scheduled with this `why` - when several
    /// plugins share an event kind (`intero`), each cancels only its own crossings.
    pub fn cancel_pending_why(&mut self, subject: EntityId, kind: &'static str, why: &'static str) -> &mut Self { self.cancels_for_why.push((subject, kind, why)); self }
    pub fn note(&mut self, s: impl Into<String>) -> &mut Self { self.notes.push(Note::Info(s.into())); self }
    pub fn is_empty(&self) -> bool { self.changes.is_empty() && self.schedules.is_empty() && self.cancels.is_empty() && self.cancels_for.is_empty() && self.cancels_for_why.is_empty() && self.notes.is_empty() }
    pub fn merge(&mut self, other: Out) {
        self.changes.extend(other.changes);
        self.schedules.extend(other.schedules);
        self.cancels.extend(other.cancels);
        self.cancels_for.extend(other.cancels_for);
        self.cancels_for_why.extend(other.cancels_for_why);
        self.notes.extend(other.notes);
    }
}
