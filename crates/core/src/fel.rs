//! The future event list. Ordered by `(at, prio, seq)`; `seq` is assigned at schedule
//! time and doubles as the `EventId`. A `BTreeMap` gives O(log n) push/pop/cancel with no
//! dependency and deterministic order (ARCHITECTURE decision 11).
use crate::event::{Event, EventKind, Handle, Payload, Schedule};
use crate::id::{EntityId, EventId};
use crate::time::SimTime;
use indexmap::IndexMap;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone)]
struct Pending {
    kind: EventKind,
    kind_name: &'static str,
    subject: EntityId,
    payload: Payload,
    why: &'static str,
    cause: Option<EventId>,
}

type Key = (SimTime, u8, u64);

/// One pending event as THE EPOCH carries it: kinds by name, `why` as text (interned back on restore).
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PendingRow { pub at: SimTime, pub prio: u8, pub seq: u64, pub kind: String, pub subject: EntityId, pub payload: Vec<u8>, pub why: String, pub cause: Option<EventId> }

#[derive(Default)]
pub struct Fel {
    queue: BTreeMap<Key, Pending>,
    /// pending keys per subject - `cancel_for` is the most common FEL operation and must not scan
    /// (a `BTreeSet`, not a `Vec`: one subject may own a million pending events)
    by_subject: IndexMap<EntityId, BTreeSet<Key>>,
    seq: u64,
}

impl Fel {
    pub fn new() -> Fel { Fel::default() }
    pub fn push(&mut self, s: Schedule, kind: EventKind, cause: Option<EventId>) -> Handle {
        self.seq += 1;
        let h = Handle { at: s.at, prio: s.prio, seq: self.seq };
        let key = (s.at, s.prio, self.seq);
        self.by_subject.entry(s.subject).or_default().insert(key);
        self.queue.insert(key, Pending { kind, kind_name: s.kind, subject: s.subject, payload: s.payload, why: s.why, cause });
        h
    }
    fn forget(&mut self, subject: EntityId, key: Key) {
        if let Some(v) = self.by_subject.get_mut(&subject) {
            v.remove(&key);
            if v.is_empty() { self.by_subject.swap_remove(&subject); }
        }
    }
    /// Cancel one handle. Returns the cancelled event's `why`, if it was pending.
    pub fn cancel(&mut self, h: Handle) -> Option<&'static str> {
        let key = (h.at, h.prio, h.seq);
        match self.queue.remove(&key) {
            Some(p) => { self.forget(p.subject, key); Some(p.why) }
            None => None,
        }
    }
    /// Cancel every pending event of `kind` (or any kind) whose subject is `e`. Returns their `why`s.
    pub fn cancel_for(&mut self, e: EntityId, kind: Option<EventKind>) -> Vec<&'static str> {
        let Some(keys) = self.by_subject.get(&e).map(|v| v.iter().copied().collect::<Vec<_>>()) else { return Vec::new() };
        let mut whys = Vec::new();
        for k in keys {
            let hit = self.queue.get(&k).is_some_and(|p| kind.is_none_or(|kk| p.kind == kk));
            if hit { if let Some(p) = self.queue.remove(&k) { self.forget(e, k); whys.push(p.why); } }
        }
        whys
    }
    /// Cancel pending events of `kind` for `e` whose `why` matches - a plugin cancels only its own
    /// crossings when several plugins share an event kind. Returns their `why`s.
    pub fn cancel_for_why(&mut self, e: EntityId, kind: EventKind, why: &str) -> Vec<&'static str> {
        let Some(keys) = self.by_subject.get(&e).map(|v| v.iter().copied().collect::<Vec<_>>()) else { return Vec::new() };
        let mut whys = Vec::new();
        for k in keys {
            let hit = self.queue.get(&k).is_some_and(|p| p.kind == kind && p.why == why);
            if hit { if let Some(p) = self.queue.remove(&k) { self.forget(e, k); whys.push(p.why); } }
        }
        whys
    }
    pub fn pop(&mut self) -> Option<Event> {
        let (k, p) = self.queue.pop_first()?;
        self.forget(p.subject, k);
        Some(Event { id: EventId(k.2), at: k.0, prio: k.1, kind: p.kind, kind_name: p.kind_name, subject: p.subject, cause: p.cause, payload: p.payload, why: p.why })
    }
    pub fn peek_time(&self) -> Option<SimTime> { self.queue.keys().next().map(|k| k.0) }
    pub fn len(&self) -> usize { self.queue.len() }
    pub fn is_empty(&self) -> bool { self.queue.is_empty() }
    pub fn seq(&self) -> u64 { self.seq }
    /// (handle, kind name, subject, why) in order - for `bodysim inspect`.
    pub fn iter(&self) -> impl Iterator<Item = (Handle, &'static str, EntityId, &'static str)> + '_ {
        self.queue.iter().map(|(k, p)| (Handle { at: k.0, prio: k.1, seq: k.2 }, p.kind_name, p.subject, p.why))
    }
    pub fn pending_for(&self, e: EntityId, kind: Option<EventKind>) -> Vec<Handle> {
        let mut v: Vec<Handle> = self.by_subject.get(&e).into_iter().flatten()
            .filter(|k| self.queue.get(k).is_some_and(|p| kind.is_none_or(|kk| p.kind == kk)))
            .map(|k| Handle { at: k.0, prio: k.1, seq: k.2 }).collect();
        v.sort();
        v
    }
    /// Every pending event, in order (THE EPOCH).
    pub fn rows(&self) -> Vec<PendingRow> {
        self.queue.iter().map(|((at, prio, seq), p)| PendingRow { at: *at, prio: *prio, seq: *seq, kind: p.kind_name.to_string(), subject: p.subject, payload: p.payload.0.clone(), why: p.why.to_string(), cause: p.cause }).collect()
    }
    /// The queue as written, with the sequence it had: event ids key the RNG streams and never restart. `why`
    /// strings are interned (a handful per epoch; the engine keeps them as `&'static str`).
    pub fn restore_rows(&mut self, rows: Vec<(PendingRow, EventKind, &'static str)>, seq: u64) {
        self.queue.clear();
        self.by_subject.clear();
        let mut whys: IndexMap<String, &'static str> = IndexMap::new();
        for (r, kind, kind_name) in rows {
            let why: &'static str = match whys.get(&r.why) { Some(w) => w, None => { let w: &'static str = Box::leak(r.why.clone().into_boxed_str()); whys.insert(r.why.clone(), w); w } };
            let key = (r.at, r.prio, r.seq);
            self.by_subject.entry(r.subject).or_default().insert(key);
            self.queue.insert(key, Pending { kind, kind_name, subject: r.subject, payload: Payload(r.payload), why, cause: r.cause });
        }
        self.seq = seq;
    }
    /// A fresh event id outside the queue (the epoch's own founding events).
    pub fn next_id(&mut self) -> EventId { self.seq += 1; EventId(self.seq) }
    pub fn hash_into(&self, h: &mut blake3::Hasher) {
        h.update(b"fel");
        h.update(&self.seq.to_le_bytes());
        h.update(&(self.queue.len() as u64).to_le_bytes());
        for ((at, prio, seq), p) in &self.queue {
            h.update(&at.0.to_le_bytes());
            h.update(&[*prio]);
            h.update(&seq.to_le_bytes());
            h.update(&p.kind.0.to_le_bytes());
            h.update(&p.subject.index.to_le_bytes());
            h.update(&p.subject.gen.to_le_bytes());
            h.update(&p.payload.0);
        }
    }
}
