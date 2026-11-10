//! THE FRESH EPOCH. The replay law says the state is a fold of the log and the log replays only
//! under the content it was written with. That froze the engine: a new object, a changed constant, a fixed rule failed
//! every old log at its first checkpoint, and both houses crash-looped the one time it was tried. The epoch is the
//! door out. `bodysim epoch` folds the log to its end and writes THE WHOLE STATE as one record at the head of a fresh
//! log, under this binary's content hash; the old log is archived as history. A later binary with new content runs
//! `epoch` again: it restores the state (any content hash, on purpose, by that one command), adds what its content has
//! that the world lacks (new entities, new components on old entities, new edges), gives the systems that did not
//! exist before their `init`, and writes its own fresh log. `serve` and `load` still REFUSE a log whose header hash is
//! not theirs: adoption is a command, never an accident.
//!
//! What the state carries, and why in that shape: components by KIND NAME in RON (self-describing, so a component may
//! grow a `#[serde(default)]` field between epochs), edges and pending events by kind NAME (ids are sorted-name
//! indices and a new kind shifts them), the entity table, the laws, the event counter and the event sequence (event
//! ids key the RNG streams, so they must continue, never restart).
use crate::component::{AnyComponent, Component, Signature};
use crate::edges::Edge;
use crate::engine::Engine;
use crate::event::{Event, Payload};
use crate::fel::PendingRow;
use crate::id::{EntityId, EventId};
use crate::log::{Envelope, LogWriter, Record};
use crate::proposal::{Change, EdgeOp, Verdict};
use crate::registry::{Blueprint, EV_INIT};
use crate::time::SimTime;
use crate::world::Label;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const EPOCH_VERSION: u16 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EpochState {
    pub version: u16,
    pub now: SimTime,
    /// `counters.events` at the fold: the next External's `after_events` continues from here
    pub events: u64,
    /// the FEL sequence: event ids key the RNG streams and must never restart
    pub seq: u64,
    /// postcard of the entity table (core-owned, versioned with core)
    pub entities: Vec<u8>,
    /// (component kind name, RON rows of `(entity, changed_at, component)`)
    pub stores: Vec<(String, String)>,
    /// (relation kind name, edge)
    pub edges: Vec<(String, Edge)>,
    pub fel: Vec<PendingRow>,
    pub laws: BTreeMap<String, f64>,
    /// the systems of the binary that wrote this state: a system not in this list is new and gets its `init`
    pub systems: Vec<String>,
    /// the writer's state hash, so the reader can prove the restore
    pub hash: [u8; 32],
}

/// What `adopt` did to an old state under new content.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Adoption {
    pub spawned: Vec<String>,
    pub components_added: Vec<(String, String)>,
    pub edges_opened: Vec<(String, String, String)>,
    pub systems_inited: Vec<String>,
    pub events_dropped: Vec<(String, String)>,
    pub stores_missing: Vec<String>,
}
impl Adoption {
    pub fn is_empty(&self) -> bool { self.spawned.is_empty() && self.components_added.is_empty() && self.edges_opened.is_empty() && self.systems_inited.is_empty() && self.events_dropped.is_empty() }
}

impl Engine {
    /// The whole state, as one record's worth of bytes.
    pub fn epoch_state(&self) -> Result<EpochState, String> {
        let w = &self.world;
        let mut stores = Vec::new();
        for (name, k) in w.kinds.components.iter() {
            let s = w.store(*k);
            if s.is_empty() { continue; }
            stores.push((name.to_string(), s.snapshot_ron()?));
        }
        let edges = w.edges.all().iter().map(|e| (w.kinds.relation_name(e.kind).to_string(), e.clone())).collect();
        Ok(EpochState {
            version: EPOCH_VERSION,
            now: w.now,
            events: self.counters.events,
            seq: self.fel.seq(),
            entities: w.entities.snapshot(),
            stores,
            edges,
            fel: self.fel.rows(),
            laws: w.laws.clone(),
            systems: self.systems.iter().map(|s| s.desc.name.to_string()).collect(),
            hash: self.hash(),
        })
    }

    /// An engine standing at a written state. The blueprint may be newer than the writer's: kinds are matched by
    /// name; a component kind the blueprint no longer has is an error (its rows would be lost silently otherwise);
    /// a pending event whose kind is gone is dropped and named in the log.
    pub fn from_epoch(bp: &Blueprint, seed: u64, log: LogWriter, checkpoint_every: u64, st: &EpochState) -> Result<(Engine, Adoption), String> {
        if st.version != EPOCH_VERSION { return Err(format!("epoch state version {} (this binary reads {EPOCH_VERSION})", st.version)); }
        let mut e = Engine::from_blueprint_bare(bp, seed, log, checkpoint_every)?;
        let mut ad = Adoption::default();
        e.world.now = st.now;
        e.counters.events = st.events;
        e.world.entities.restore(&st.entities)?;
        for (name, ron) in &st.stores {
            let Some(k) = e.world.kinds.component(name) else { ad.stores_missing.push(name.clone()); continue };
            e.world.store_mut(k).restore_ron(ron).map_err(|err| format!("restoring `{name}`: {err}"))?;
        }
        if !ad.stores_missing.is_empty() { return Err(format!("this binary has no component kind {:?}; a removed kind needs its own migration", ad.stores_missing)); }
        e.world.rebuild_indexes();
        let mut edge_rows = Vec::with_capacity(st.edges.len());
        for (name, edge) in &st.edges {
            let Some(k) = e.world.kinds.relation(name) else { return Err(format!("this binary has no relation kind `{name}`")) };
            let mut ed = edge.clone();
            ed.kind = k;
            edge_rows.push(ed);
        }
        e.world.edges.restore_rows(edge_rows);
        let mut kept = Vec::with_capacity(st.fel.len());
        for row in &st.fel {
            match (e.world.kinds.event(&row.kind), e.world.kinds.event_static(&row.kind)) {
                (Some(k), Some(name)) => kept.push((row.clone(), k, name)),
                _ => ad.events_dropped.push((row.kind.clone(), row.why.clone())),
            }
        }
        e.fel.restore_rows(kept, st.seq);
        e.world.laws = st.laws.clone();
        for (kind, why) in &ad.events_dropped {
            e.log.append(Record::Note { event: EventId::GENESIS, system: "epoch".into(), text: format!("pending `{kind}` ({why}) dropped: this binary has no such event kind") });
        }
        Ok((e, ad))
    }

    /// New content into an old world: entities the scenes name that do not exist are spawned; components a scene or
    /// its proto gives an existing entity that it lacks are inserted; edges the scenes name that are not open are
    /// opened; systems the writer did not have get their `init`. Everything that existed keeps its values.
    pub fn adopt_content(&mut self, bp: &Blueprint, ad: &mut Adoption, writer_systems: &[String]) -> Result<(), String> {
        for scene in &bp.scenes { self.load_scene_missing(scene, ad)?; }
        let k_init = self.world.kinds.event(EV_INIT).ok_or("no init event")?;
        let new_systems: Vec<usize> = self.systems.iter().enumerate()
            .filter(|(_, s)| !writer_systems.iter().any(|w| w == s.desc.name) && s.handles.contains(&k_init))
            .map(|(i, _)| i).collect();
        for si in new_systems {
            let id = self.fel.next_id();
            let ev = Event { id, at: self.world.now, prio: crate::event::PRIO_FIRST, kind: k_init, kind_name: EV_INIT, subject: EntityId::WORLD, cause: None, payload: Payload::none(), why: "epoch: a new system's first wake" };
            self.log.append(Record::Event { id, at: ev.at, kind: EV_INIT.into(), subject: EntityId::WORLD, cause: None, why: ev.why.into() });
            self.run_handler(si, &ev);
            ad.systems_inited.push(self.systems[si].desc.name.to_string());
        }
        Ok(())
    }

    /// `load_scene`, adding only: an entity that exists keeps every component it has; one that does not is spawned
    /// whole; an edge that is open stays; the world entity gets the resources it lacks.
    fn load_scene_missing(&mut self, scene: &crate::content::LoadedScene, ad: &mut Adoption) -> Result<(), String> {
        let mut ids: indexmap::IndexMap<String, EntityId> = indexmap::IndexMap::new();
        for spec in &scene.entities {
            let existing = if spec.name == "world" { Some(EntityId::WORLD) } else { self.world.find(&spec.name) };
            let mut boxed: Vec<Box<dyn AnyComponent>> = Vec::new();
            if let Some(p) = &spec.proto {
                let proto = self.world.protos.get(p).ok_or_else(|| format!("unknown proto {p}"))?;
                boxed.extend(proto.components.iter().cloned());
            }
            for c in &spec.components {
                boxed.retain(|b| b.kind_name() != c.kind_name());
                boxed.push(c.clone());
            }
            match existing {
                Some(e) => {
                    for c in boxed {
                        let Some(k) = self.world.kinds.component(c.kind_name()) else { return Err(format!("unknown component kind {}", c.kind_name())) };
                        if self.world.has(e, k) { continue; }
                        let kind = c.kind_name().to_string();
                        let mut committed = Vec::new();
                        match self.apply_change(e, Change::Insert(c), None, EventId::GENESIS, &[], &mut committed) {
                            Verdict::Rejected(v) => return Err(format!("scene entity `{}`: adding `{kind}` refused: {v}", spec.name)),
                            _ => ad.components_added.push((spec.name.clone(), kind)),
                        }
                    }
                    ids.insert(spec.name.clone(), e);
                }
                None => {
                    if !boxed.iter().any(|b| b.kind_name() == Label::NAME) { boxed.push(Box::new(Label { name: spec.name.clone() })); }
                    let e = self.world.entities.reserve();
                    let mut committed = Vec::new();
                    match self.apply_change(e, Change::Spawn { components: boxed }, None, EventId::GENESIS, &[e], &mut committed) {
                        Verdict::Applied | Verdict::Noop => {}
                        Verdict::Rejected(v) => { self.world.entities.release(e); return Err(format!("scene entity `{}` refused: {v}", spec.name)); }
                    }
                    ad.spawned.push(spec.name.clone());
                    ids.insert(spec.name.clone(), e);
                }
            }
        }
        for spec in &scene.entities {
            let src = ids[&spec.name];
            for ed in &spec.edges {
                let dst = ids.get(&ed.dst).copied().or_else(|| self.world.find(&ed.dst)).ok_or_else(|| format!("entity `{}` edge to unknown `{}`", spec.name, ed.dst))?;
                let kind = self.world.kinds.relation_static(&ed.kind).ok_or_else(|| format!("unknown relation {}", ed.kind))?;
                let rk = self.world.kinds.relation(&ed.kind).unwrap();
                if self.world.edges.open_between(src, dst, rk).is_some() { continue; }
                let mut committed = Vec::new();
                if let Verdict::Rejected(v) = self.apply_change(src, Change::Edge(EdgeOp::Open { kind, dst, strength: None }), None, EventId::GENESIS, &[], &mut committed) {
                    return Err(format!("scene edge {} -{}-> {} refused: {v}", spec.name, ed.kind, ed.dst));
                }
                ad.edges_opened.push((spec.name.clone(), ed.kind.clone(), ed.dst.clone()));
            }
        }
        Ok(())
    }
}

/// The record at index 1 of an epoch log, decoded.
pub fn epoch_of(records: &[Envelope]) -> Result<Option<EpochState>, String> {
    match records.get(1).map(|e| &e.record) {
        Some(Record::Epoch { state, .. }) => Ok(Some(postcard::from_bytes(state).map_err(|e| format!("epoch state: {e}"))?)),
        _ => Ok(None),
    }
}

/// The fresh log: header under `bp`'s content hash, the state, a checkpoint that pins it, then the founding's own
/// records (what adoption applied), so the fold is `restore` and nothing else. `path` must not exist yet.
#[allow(clippy::too_many_arguments)] // every argument is a distinct part of the log header
pub fn write_epoch_log(path: &std::path::Path, bp: &Blueprint, seed: u64, checkpoint_every: u64, created: &str, st: &EpochState, prior: &str, prior_hash: [u8; 32], prior_records: u64, founding: &[Envelope]) -> Result<(), String> {
    // a fresh file only: the prior log is archived as history, never written over (archive it first)
    let mut log = LogWriter::file_new(path).map_err(|e| match e.kind() {
        std::io::ErrorKind::AlreadyExists => format!("{} already exists: an epoch log is a new file, archive the prior one first", path.display()),
        _ => format!("{}: {e}", path.display()),
    })?;
    log.rule_versions = bp.rule_versions;
    log.append(Record::Header { seed, schema_version: crate::log::SCHEMA_VERSION, checkpoint_every, created: created.to_string(), content_hash: bp.content_hash, plugins: bp.plugins.iter().map(|p| p.to_string()).collect() });
    let bytes = postcard::to_stdvec(st).map_err(|e| e.to_string())?;
    log.append(Record::Epoch { at: st.now, events: st.events, prior: prior.to_string(), prior_hash, prior_records, state: bytes });
    log.append(Record::Checkpoint { at: st.now, events: st.events, hash: st.hash });
    for env in founding { log.append(env.record.clone()); }
    log.flush();
    Ok(())
}

impl Signature {
    /// The signature an entity has from what the stores hold.
    pub(crate) fn of_stores(w: &crate::world::World, e: EntityId) -> Signature {
        let mut s = Signature::default();
        for (_, k) in w.kinds.components.iter() { if w.store(*k).has(e) { s.insert(*k); } }
        s
    }
}
