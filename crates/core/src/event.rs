//! Events and schedules. Every scheduled wake carries `why` (provenance is not optional).
use crate::id::{EntityId, EventId};
use crate::time::{SimTime, Span};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug, Default)]
pub struct EventKind(pub u16);

/// postcard bytes; the handling system deserializes into its own type - core never knows the shape.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Payload(pub Vec<u8>);
impl Payload {
    pub fn none() -> Payload { Payload(Vec::new()) }
    pub fn of<T: Serialize>(v: &T) -> Payload { Payload(postcard::to_stdvec(v).expect("postcard")) }
    pub fn decode<T: DeserializeOwned>(&self) -> Option<T> { postcard::from_bytes(&self.0).ok() }
    pub fn is_empty(&self) -> bool { self.0.is_empty() }
}

pub const PRIO_DEFAULT: u8 = 128;
pub const PRIO_FIRST: u8 = 0;
pub const PRIO_LAST: u8 = 255;

#[derive(Clone, Debug)]
pub struct Event {
    pub id: EventId,
    pub at: SimTime,
    pub prio: u8,
    pub kind: EventKind,
    pub kind_name: &'static str,
    pub subject: EntityId,
    pub cause: Option<EventId>,
    pub payload: Payload,
    pub why: &'static str,
}
impl Event {
    pub fn decode<T: DeserializeOwned>(&self) -> Option<T> { self.payload.decode() }
    pub fn is(&self, kind: &str) -> bool { self.kind_name == kind }
}

/// A request to wake at `at`. `kind` is the registered event NAME; resolved by the engine.
#[derive(Clone, Debug)]
pub struct Schedule {
    pub at: SimTime,
    pub prio: u8,
    pub kind: &'static str,
    pub subject: EntityId,
    pub payload: Payload,
    /// LAW: every scheduled wake carries why it exists. Also the key for crossing counters.
    pub why: &'static str,
}
impl Schedule {
    pub fn new(at: SimTime, kind: &'static str, subject: EntityId, why: &'static str) -> Schedule {
        Schedule { at, prio: PRIO_DEFAULT, kind, subject, payload: Payload::none(), why }
    }
    pub fn after(now: SimTime, delay: Span, kind: &'static str, subject: EntityId, why: &'static str) -> Schedule {
        Schedule::new(now + delay, kind, subject, why)
    }
    pub fn with_payload(mut self, p: Payload) -> Schedule { self.payload = p; self }
    pub fn payload<T: Serialize>(mut self, v: &T) -> Schedule { self.payload = Payload::of(v); self }
    pub fn prio(mut self, p: u8) -> Schedule { self.prio = p; self }
}

/// For cancel/reschedule - the most common FEL operation.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Handle { pub at: SimTime, pub prio: u8, pub seq: u64 }
impl Handle {
    pub fn id(&self) -> EventId { EventId(self.seq) }
}
