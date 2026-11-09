//! # bodysim-core - the substrate
//!
//! A discrete-event engine with no global tick. Six primitives: entity (an id), component
//! (registered by NAME, stored in a sparse set), rule (bound to a component signature),
//! system (declares reads/writes, PROPOSES, never mutates), relation (a first-class
//! bitemporal edge), ledger (the log is the truth; state is a fold).
//!
//! `core` contains NO domain words. Kinds are registered by plugins; ids are assigned at
//! `Registry::finalize` in sorted-name order so they are stable across runs.
//!
#![forbid(unsafe_code)]

pub mod time;
pub mod id;
pub mod value;
pub mod relax;
pub mod component;
pub mod entities;
pub mod edges;
pub mod event;
pub mod fel;
pub mod predicate;
pub mod proposal;
pub mod rule;
pub mod constants;
pub mod rng;
pub mod log;
pub mod world;
pub mod registry;
pub mod content;
pub mod engine;
pub mod epoch;
pub mod fixture;

pub use indexmap;
pub use postcard;
pub use rand_chacha;
pub use ron;
pub use serde;

pub mod prelude {
    pub use crate::component::{downcast, AnyComponent, Component, Signature, SparseSet};
    pub use crate::constants::{ConstRow, ConstStatus, Constants};
    pub use crate::edges::{Edge, Edges, RelKind, RelTraits};
    pub use crate::engine::{Ctx, Engine, RunReport, StepReport};
    pub use crate::event::{Event, EventKind, Handle, Payload, Schedule};
    pub use crate::id::{EdgeId, EntityId, EventId, FieldId, ProtoId, RuleId, SystemId};
    pub use crate::log::{LogWriter, Record};
    pub use crate::predicate::{Bindings, Cmp, Effect, EffectPayload, Predicate, Ref};
    pub use crate::proposal::{Change, EdgeOp, Note, Out, Proposal, Verdict, Violation};
    pub use crate::registry::{
        BoundedDecl, Defect, Invariant, Mechanism, Plugin, Registry, RestBound, SystemDescriptor, EV_LAW,
    };
    pub use crate::world::LawSet;
    pub use crate::relax::{Bateman, Expm2, Linear, Logistic, PiecewiseLinear, Reach, Relax, Trajectory};
    pub use crate::rng::{bernoulli, exponential, normal, uniform, weibull};
    pub use crate::rule::{DeclRule, Dir, FieldChange, Rule, RuleOp, RuleView};
    pub use crate::time::{CivilEpoch, CivilTime, SimTime, Span};
    pub use crate::value::{FieldValue, Value, ValueError};
    pub use crate::world::{Kinds, Label, World};
    pub use crate::{component, value_enum};
}
