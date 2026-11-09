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
    /// pending keys per (subject, kind): cancelling one kind must not walk every event the subject
    /// owns (a year of meals injected up front made each cancel walk thousands of them)
    by_kind: IndexMap<(EntityId, EventKind), BTreeSet<Key>>,
    seq: u64,
}

impl Fel {
    pub fn new() -> Fel { Fel::default() }
    pub fn push(&mut self, s: Schedule, kind: EventKind, cause: Option<EventId>) -> Handle {
        self.seq += 1;
        let h = Handle { at: s.at, prio: s.prio, seq: self.seq };
        let key = (s.at, s.prio, self.seq);
        self.by_subject.entry(s.subject).or_default().insert(key);
        self.by_kind.entry((s.subject, kind)).or_default().insert(key);
        self.queue.insert(key, Pending { kind, kind_name: s.kind, subject: s.subject, payload: s.payload, why: s.why, cause });
        h
    }
    fn forget(&mut self, subject: EntityId, kind: EventKind, key: Key) {
        if let Some(v) = self.by_subject.get_mut(&subject) {
            v.remove(&key);
            if v.is_empty() { self.by_subject.swap_remove(&subject); }
        }
        if let Some(v) = self.by_kind.get_mut(&(subject, kind)) {
            v.remove(&key);
            if v.is_empty() { self.by_kind.swap_remove(&(subject, kind)); }
        }
    }
    /// The pending keys of `e` (of one kind, or all), in queue order.
    fn keys_for(&self, e: EntityId, kind: Option<EventKind>) -> Vec<Key> {
        match kind {
            Some(k) => self.by_kind.get(&(e, k)).map(|v| v.iter().copied().collect()).unwrap_or_default(),
            None => self.by_subject.get(&e).map(|v| v.iter().copied().collect()).unwrap_or_default(),
        }
    }
    /// Cancel one handle. Returns the cancelled event's `why`, if it was pending.
    pub fn cancel(&mut self, h: Handle) -> Option<&'static str> {
        let key = (h.at, h.prio, h.seq);
        match self.queue.remove(&key) {
            Some(p) => { self.forget(p.subject, p.kind, key); Some(p.why) }
            None => None,
        }
    }
    /// Cancel every pending event of `kind` (or any kind) whose subject is `e`. Returns their `why`s.
    pub fn cancel_for(&mut self, e: EntityId, kind: Option<EventKind>) -> Vec<&'static str> {
        let mut whys = Vec::new();
        for k in self.keys_for(e, kind) {
            if let Some(p) = self.queue.remove(&k) { self.forget(e, p.kind, k); whys.push(p.why); }
        }
        whys
    }
    /// Cancel pending events of `kind` for `e` whose `why` matches - a plugin cancels only its own
    /// crossings when several plugins share an event kind. Returns their `why`s.
    pub fn cancel_for_why(&mut self, e: EntityId, kind: EventKind, why: &str) -> Vec<&'static str> {
        let mut whys = Vec::new();
        for k in self.keys_for(e, Some(kind)) {
            if self.queue.get(&k).is_some_and(|p| p.why == why) {
                if let Some(p) = self.queue.remove(&k) { self.forget(e, kind, k); whys.push(p.why); }
            }
        }
        whys
    }
    pub fn pop(&mut self) -> Option<Event> {
        let (k, p) = self.queue.pop_first()?;
        self.forget(p.subject, p.kind, k);
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
        self.keys_for(e, kind).into_iter().map(|k| Handle { at: k.0, prio: k.1, seq: k.2 }).collect()
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
        self.by_kind.clear();
        let mut whys: IndexMap<String, &'static str> = IndexMap::new();
        for (r, kind, kind_name) in rows {
            let why: &'static str = match whys.get(&r.why) { Some(w) => w, None => { let w: &'static str = Box::leak(r.why.clone().into_boxed_str()); whys.insert(r.why.clone(), w); w } };
            let key = (r.at, r.prio, r.seq);
            self.by_subject.entry(r.subject).or_default().insert(key);
            self.by_kind.entry((r.subject, kind)).or_default().insert(key);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Span;

    fn sched(fel: &mut Fel, mins: i64, kind: u16, who: u32, why: &'static str) {
        let s = Schedule { at: SimTime::ZERO + Span::mins(mins), prio: 0, kind: "k", subject: EntityId::new(who, 0), payload: Payload::none(), why };
        fel.push(s, EventKind(kind), None);
    }

    #[test]
    fn cancelling_one_kind_touches_only_that_kind_and_the_index_stays_whole() {
        let mut fel = Fel::new();
        let (a, b) = (EntityId::new(1, 0), EntityId::new(2, 0));
        for m in 0..1000 { sched(&mut fel, m, 1, 1, "meal"); }
        sched(&mut fel, 5, 2, 1, "check");
        sched(&mut fel, 3, 2, 1, "other");
        sched(&mut fel, 4, 2, 2, "check");
        assert_eq!(fel.pending_for(a, Some(EventKind(2))).iter().map(|h| h.at).collect::<Vec<_>>(),
                   vec![SimTime::ZERO + Span::mins(3), SimTime::ZERO + Span::mins(5)], "in queue order");
        assert_eq!(fel.cancel_for_why(a, EventKind(2), "check"), vec!["check"]);
        assert_eq!(fel.cancel_for(a, Some(EventKind(2))), vec!["other"]);
        assert_eq!(fel.len(), 1001, "the other subject and the other kind are untouched");
        assert_eq!(fel.pending_for(b, None).len(), 1);
        while let Some(ev) = fel.pop() { assert!(ev.kind == EventKind(1) || ev.subject == b); }
        assert!(fel.by_subject.is_empty() && fel.by_kind.is_empty(), "nothing left behind in either index");
    }

    #[test]
    fn restored_rows_rebuild_the_kind_index() {
        let mut fel = Fel::new();
        for m in 0..10 { sched(&mut fel, m, (m % 2) as u16, 1, "x"); }
        let rows = fel.rows().into_iter().map(|r| { let k = EventKind((r.at.0 / 60_000_000_000 % 2) as u16); (r, k, "k") }).collect();
        let mut back = Fel::new();
        back.restore_rows(rows, fel.seq());
        assert_eq!(back.cancel_for(EntityId::new(1, 0), Some(EventKind(1))).len(), 5);
        assert_eq!(back.len(), 5);
    }
}
