//! Identifiers. Entities are generational (reuse-safe); everything else is a plain index.
use serde::{Deserialize, Serialize};
use std::fmt;

/// A generational entity id. Just an id: there is no special case for people.
#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
pub struct EntityId { pub index: u32, pub gen: u32 }

impl EntityId {
    /// The world entity: index 0, allocated first, never despawned. Resources live on it.
    pub const WORLD: EntityId = EntityId { index: 0, gen: 0 };
    pub fn new(index: u32, gen: u32) -> EntityId { EntityId { index, gen } }
}
impl fmt::Display for EntityId { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "e{}.{}", self.index, self.gen) } }
impl fmt::Debug for EntityId { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self}") } }

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident, $t:ty, $prefix:literal) => {
        $(#[$m])*
        #[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Default)]
        pub struct $name(pub $t);
        impl fmt::Display for $name { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}{}", $prefix, self.0) } }
        impl fmt::Debug for $name { fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{self}") } }
    };
}

id_type!(/// Event ids are the FEL sequence number: unique, monotonic in scheduling order.
    EventId, u64, "ev");
id_type!(EdgeId, u64, "edge");
id_type!(SystemId, u16, "sys");
id_type!(RuleId, u16, "rule");
id_type!(FieldId, u16, "f");
id_type!(ProtoId, u16, "proto");

impl SystemId {
    /// The engine itself (scene loading, invariants). Bypasses ownership, never rules.
    pub const ENGINE: SystemId = SystemId(u16::MAX);
}
impl EventId {
    /// The pseudo-event that causes everything applied at build time (scene load).
    pub const GENESIS: EventId = EventId(0);
}
