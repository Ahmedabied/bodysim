//! Relations are first-class, bitemporal edges (-24): never mutated, closed and reopened.
//! Both forward and reverse indices exist from day one.
use crate::id::{EdgeId, EntityId, EventId};
use crate::relax::Relax;
use crate::time::SimTime;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug, Default)]
pub struct RelKind(pub u16);

/// Declared at registration, by hand, after flecs's taxonomy.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelTraits {
    /// at most one open edge of this kind per source; opening a second closes the first
    pub exclusive: bool,
    /// stored once, queried from both ends
    pub symmetric: bool,
    pub transitive: bool,
    /// opening an edge that would close a cycle is rejected
    pub acyclic: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Edge {
    pub id: EdgeId,
    pub kind: RelKind,
    pub src: EntityId,
    pub dst: EntityId,
    /// decaying relations (closeness, trust) are anchors, not numbers
    pub strength: Option<Relax>,
    pub born: SimTime,
    pub valid_to: Option<SimTime>,
    pub cause: EventId,
    pub superseded_by: Option<EdgeId>,
}
impl Edge {
    pub fn is_open(&self) -> bool { self.valid_to.is_none() }
    pub fn was_open_at(&self, t: SimTime) -> bool { self.born <= t && self.valid_to.is_none_or(|v| t < v) }
}

#[derive(Default)]
pub struct Edges {
    rows: Vec<Edge>,
    /// every edge ever, by end (the bitemporal read)
    by_src: IndexMap<EntityId, Vec<EdgeId>>,
    by_dst: IndexMap<EntityId, Vec<EdgeId>>,
    /// only the live edges, by end - pruned on close, so a live read never walks history
    open_src: IndexMap<EntityId, Vec<EdgeId>>,
    open_dst: IndexMap<EntityId, Vec<EdgeId>>,
    traits: Vec<RelTraits>,
}

impl Edges {
    pub fn new(traits: Vec<RelTraits>) -> Edges { Edges { traits, ..Default::default() } }
    pub fn traits(&self, k: RelKind) -> RelTraits { self.traits.get(k.0 as usize).copied().unwrap_or_default() }
    pub fn get(&self, id: EdgeId) -> Option<&Edge> { self.rows.get(id.0 as usize) }
    pub fn all(&self) -> &[Edge] { &self.rows }
    pub fn count_open(&self) -> usize { self.rows.iter().filter(|e| e.is_open()).count() }

    /// Live edges of `kind` leaving `src` (both ends if symmetric).
    pub fn open(&self, src: EntityId, kind: RelKind) -> Vec<&Edge> {
        let mut v: Vec<&Edge> = self.open_src.get(&src).into_iter().flatten()
            .map(|id| &self.rows[id.0 as usize]).filter(|e| e.kind == kind).collect();
        if self.traits(kind).symmetric {
            v.extend(self.open_dst.get(&src).into_iter().flatten()
                .map(|id| &self.rows[id.0 as usize]).filter(|e| e.kind == kind && e.src != src));
        }
        v
    }
    /// Live edges of `kind` arriving at `dst` (both ends if symmetric).
    pub fn open_to(&self, dst: EntityId, kind: RelKind) -> Vec<&Edge> {
        let mut v: Vec<&Edge> = self.open_dst.get(&dst).into_iter().flatten()
            .map(|id| &self.rows[id.0 as usize]).filter(|e| e.kind == kind).collect();
        if self.traits(kind).symmetric {
            v.extend(self.open_src.get(&dst).into_iter().flatten()
                .map(|id| &self.rows[id.0 as usize]).filter(|e| e.kind == kind && e.dst != dst));
        }
        v
    }
    pub fn open_between(&self, src: EntityId, dst: EntityId, kind: RelKind) -> Option<&Edge> {
        let sym = self.traits(kind).symmetric;
        self.open(src, kind).into_iter().find(|e| e.dst == dst || (sym && e.src == dst))
    }
    /// The other end of a live edge, seen from `me`.
    pub fn other_end(&self, e: &Edge, me: EntityId) -> EntityId { if e.src == me { e.dst } else { e.src } }
    /// Bitemporal read: what was true at `t`. "What was true on Tuesday."
    pub fn at(&self, t: SimTime, src: EntityId, kind: RelKind) -> Vec<&Edge> {
        self.by_src.get(&src).into_iter().flatten()
            .map(|id| &self.rows[id.0 as usize]).filter(|e| e.kind == kind && e.was_open_at(t)).collect()
    }
    /// All live edges touching `e`, either end.
    pub fn touching(&self, e: EntityId) -> Vec<EdgeId> {
        let mut v: Vec<EdgeId> = self.open_src.get(&e).into_iter().flatten().copied().collect();
        v.extend(self.open_dst.get(&e).into_iter().flatten().copied());
        v.sort();
        v.dedup();
        v
    }
    /// Would `src → dst` close a cycle over live edges of `kind`?
    pub fn would_cycle(&self, kind: RelKind, src: EntityId, dst: EntityId) -> bool {
        if src == dst { return true; }
        let mut stack = vec![dst];
        let mut seen: Vec<EntityId> = Vec::new();
        while let Some(n) = stack.pop() {
            if n == src { return true; }
            if seen.contains(&n) { continue; }
            seen.push(n);
            for e in self.open(n, kind) { stack.push(e.dst); }
        }
        false
    }
    pub(crate) fn open_edge(&mut self, kind: RelKind, src: EntityId, dst: EntityId, strength: Option<Relax>, now: SimTime, cause: EventId) -> EdgeId {
        let id = EdgeId(self.rows.len() as u64);
        self.rows.push(Edge { id, kind, src, dst, strength, born: now, valid_to: None, cause, superseded_by: None });
        self.by_src.entry(src).or_default().push(id);
        self.by_dst.entry(dst).or_default().push(id);
        self.open_src.entry(src).or_default().push(id);
        self.open_dst.entry(dst).or_default().push(id);
        id
    }
    pub(crate) fn close_edge(&mut self, id: EdgeId, now: SimTime, superseded_by: Option<EdgeId>) -> bool {
        let (src, dst) = match self.rows.get_mut(id.0 as usize) {
            Some(e) if e.is_open() => { e.valid_to = Some(now); e.superseded_by = superseded_by; (e.src, e.dst) }
            _ => return false,
        };
        if let Some(v) = self.open_src.get_mut(&src) { v.retain(|x| *x != id); }
        if let Some(v) = self.open_dst.get_mut(&dst) { v.retain(|x| *x != id); }
        true
    }
    pub fn hash_into(&self, h: &mut blake3::Hasher) {
        h.update(b"edges");
        h.update(&(self.rows.len() as u64).to_le_bytes());
        for e in &self.rows { h.update(&postcard::to_stdvec(e).expect("postcard")); }
    }
    pub fn snapshot(&self) -> Vec<u8> { postcard::to_stdvec(&self.rows).expect("postcard") }
    pub fn restore(&mut self, bytes: &[u8]) -> Result<(), String> {
        let rows: Vec<Edge> = postcard::from_bytes(bytes).map_err(|e| e.to_string())?;
        self.restore_rows(rows);
        Ok(())
    }
    /// The rows as written, indexes rebuilt (THE EPOCH; kinds already resolved by name).
    pub fn restore_rows(&mut self, rows: Vec<Edge>) {
        self.rows.clear();
        self.by_src.clear();
        self.by_dst.clear();
        self.open_src.clear();
        self.open_dst.clear();
        for e in rows {
            self.by_src.entry(e.src).or_default().push(e.id);
            self.by_dst.entry(e.dst).or_default().push(e.id);
            if e.is_open() {
                self.open_src.entry(e.src).or_default().push(e.id);
                self.open_dst.entry(e.dst).or_default().push(e.id);
            }
            self.rows.push(e);
        }
    }
}
