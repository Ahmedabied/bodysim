//! ONE condition language, domain-free. Used by objects' preconditions/effects, rules-as-data,
//! scheduled invariants and the reader's commitments. Kinds and fields are referenced by name
//! (content is data); resolution is a map lookup at evaluation time.
use crate::event::Event;
use crate::id::EntityId;
use crate::time::{SimTime, Span};
use crate::value::Value;
use crate::world::World;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Ref { Actor, Object, Target, Id(EntityId) }

#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Cmp { Lt, Le, Eq, Ge, Gt, Ne }

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Bindings { pub actor: Option<EntityId>, pub object: Option<EntityId>, pub target: Option<EntityId> }
impl Bindings {
    pub fn resolve(&self, r: Ref) -> Option<EntityId> {
        match r { Ref::Actor => self.actor, Ref::Object => self.object, Ref::Target => self.target, Ref::Id(e) => Some(e) }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Predicate {
    Has { entity: Ref, kind: String },
    Field { entity: Ref, kind: String, field: String, op: Cmp, value: Value },
    /// a live relation exists
    Edge { kind: String, src: Ref, dst: Ref },
    /// "an event of this kind occurs" - matched against events (commitments, invariants), never point-evaluated
    Event { kind: String, subject: Ref, within: Option<Span> },
    Not(Box<Predicate>),
    All(Vec<Predicate>),
    Any(Vec<Predicate>),
}

impl Predicate {
    /// Point evaluation against the world now. `Event` predicates are false here (see `matches`).
    pub fn eval(&self, w: &World, b: &Bindings) -> bool {
        match self {
            Predicate::Has { entity, kind } => b.resolve(*entity).is_some_and(|e| w.has_named(e, kind)),
            Predicate::Field { entity, kind, field, op, value } => {
                let Some(e) = b.resolve(*entity) else { return false };
                match w.field_named(e, kind, field) { Some(v) => compare(*op, &v, value, w.now), None => false }
            }
            Predicate::Edge { kind, src, dst } => {
                let (Some(s), Some(d), Some(k)) = (b.resolve(*src), b.resolve(*dst), w.kinds.relation(kind)) else { return false };
                w.edges.open_between(s, d, k).is_some()
            }
            Predicate::Event { .. } => false,
            Predicate::Not(p) => !p.eval(w, b),
            Predicate::All(ps) => ps.iter().all(|p| p.eval(w, b)),
            Predicate::Any(ps) => ps.iter().any(|p| p.eval(w, b)),
        }
    }
    /// Does this event satisfy the predicate's `Event` leaves (combined through Not/All/Any)?
    /// Non-event leaves fall back to point evaluation.
    pub fn matches(&self, ev: &Event, w: &World, b: &Bindings, since: SimTime) -> bool {
        match self {
            Predicate::Event { kind, subject, within } => {
                ev.kind_name == kind
                    && b.resolve(*subject).is_none_or(|s| s == ev.subject)
                    && within.is_none_or(|wi| ev.at - since <= wi)
            }
            Predicate::Not(p) => !p.matches(ev, w, b, since),
            Predicate::All(ps) => ps.iter().all(|p| p.matches(ev, w, b, since)),
            Predicate::Any(ps) => ps.iter().any(|p| p.matches(ev, w, b, since)),
            other => other.eval(w, b),
        }
    }
    pub fn mentions_events(&self) -> bool {
        match self {
            Predicate::Event { .. } => true,
            Predicate::Not(p) => p.mentions_events(),
            Predicate::All(ps) | Predicate::Any(ps) => ps.iter().any(|p| p.mentions_events()),
            _ => false,
        }
    }
}

pub fn compare(op: Cmp, a: &Value, b: &Value, now: SimTime) -> bool {
    if let (Some(x), Some(y)) = (a.scalar_at(now), b.scalar_at(now)) {
        return match op { Cmp::Lt => x < y, Cmp::Le => x <= y, Cmp::Eq => x == y, Cmp::Ge => x >= y, Cmp::Gt => x > y, Cmp::Ne => x != y };
    }
    match op { Cmp::Eq => a == b, Cmp::Ne => a != b, _ => false }
}

/// Mirrors `Change`; an object's effects become proposals (bound at evaluation time).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Effect {
    Set { entity: Ref, kind: String, field: String, value: Value },
    /// `data` is the component as RON text, parsed typed by the kind's store
    Insert { entity: Ref, kind: String, data: String },
    Remove { entity: Ref, kind: String },
    EdgeOpen { kind: String, src: Ref, dst: Ref },
    EdgeClose { kind: String, src: Ref, dst: Ref },
    /// schedule the owner's event - the only way a data-driven effect touches another plugin's state
    Schedule { kind: String, subject: Ref, after: Span, #[serde(default)] payload: EffectPayload },
}

/// Small typed payloads a content effect can attach to a scheduled event. `ActorF64`/`TargetF64`
/// encode as `(EntityId, f64)` - the layout of the world's `Drink`/`Pour` payloads.
#[derive(Clone, Debug, PartialEq, Default, Serialize, Deserialize)]
pub enum EffectPayload {
    #[default]
    None,
    Bool(bool),
    F64(f64),
    Actor,
    ActorF64(f64),
    TargetF64(f64),
    /// a named thing: encodes as `(String, f64)` (the skin's `skin_scent` payload)
    TextF64(String, f64),
}
impl EffectPayload {
    pub fn encode(&self, b: &Bindings) -> Option<crate::event::Payload> {
        use crate::event::Payload;
        Some(match self {
            EffectPayload::None => Payload::none(),
            EffectPayload::Bool(x) => Payload::of(x),
            EffectPayload::F64(x) => Payload::of(x),
            EffectPayload::Actor => Payload::of(&b.actor?),
            EffectPayload::ActorF64(x) => Payload::of(&(b.actor?, *x)),
            EffectPayload::TargetF64(x) => Payload::of(&(b.target?, *x)),
            EffectPayload::TextF64(t, x) => Payload::of(&(t.clone(), *x)),
        })
    }
}
