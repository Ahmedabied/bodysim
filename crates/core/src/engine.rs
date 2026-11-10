//! The loop. Pop one event, dispatch to the systems that handle it, apply their proposals through
//! the rules, append the log, push their schedules. Single-threaded. This is the whole
//! runtime.
use crate::component::{AnyComponent, Component, ComponentKind, Signature};
use crate::event::{Event, EventKind, Payload, Schedule, PRIO_FIRST};
use crate::fel::Fel;
use crate::id::{EntityId, EventId, FieldId, SystemId};
use crate::log::{ChangeRecord, Envelope, LogWriter, Record};
use crate::proposal::{Change, EdgeOp, Note, Out, Verdict, Violation};
use crate::content::LoadedScene;
use crate::registry::{Blueprint, ResolvedBounded, ResolvedInvariant, ResolvedSystem, EV_INIT, EV_INVARIANT, EV_LAW};
use crate::rng::stream;
use crate::rule::{FieldChange, Rule, RuleOp, RuleView};
use crate::time::SimTime;
use crate::value::Value;
use crate::world::{Kinds, Label, LawSet, World};
use indexmap::IndexMap;
use rand_chacha::ChaCha8Rng;
use serde::Serialize;
use std::sync::Arc;

/// Longest chain of same-instant cause→effect events before the engine calls it a loop.
pub const MAX_SAME_INSTANT_CHAIN: u32 = 512;
/// Most events at ONE instant before the engine calls it a flood (a breadth explosion the chain
/// guard cannot see: two events each spawning two, doubling at every step).
pub const MAX_SAME_INSTANT_EVENTS: usize = 50_000;

/// What a handler gets besides `&World` and the event: keyed RNG streams, entity
/// reservation for spawns, proto instantiation.
pub struct Ctx<'a> {
    pub now: SimTime,
    pub system: SystemId,
    pub event: EventId,
    seed: u64,
    world: &'a World,
    rngs: IndexMap<EntityId, ChaCha8Rng>,
    pub(crate) reserved: Vec<EntityId>,
}
impl<'a> Ctx<'a> {
    fn new(world: &'a World, seed: u64, system: SystemId, event: EventId) -> Ctx<'a> {
        Ctx { now: world.now, system, event, seed, world, rngs: IndexMap::new(), reserved: Vec::new() }
    }
    /// The keyed stream for this (system, entity, event). Same key ⇒ same draws, every run.
    pub fn rng(&mut self, e: EntityId) -> &mut ChaCha8Rng {
        let (seed, sys, ev) = (self.seed, self.system, self.event);
        self.rngs.entry(e).or_insert_with(|| stream(seed, sys, e, ev))
    }
    /// Reserve an entity id for a `Spawn` proposal in this same `Out`.
    pub fn reserve(&mut self) -> EntityId {
        let e = self.world.entities.reserve();
        self.reserved.push(e);
        e
    }
    pub fn proto(&self, name: &str) -> Result<Vec<Box<dyn AnyComponent>>, String> { self.world.instantiate_proto(name) }
    pub fn kinds(&self) -> &Kinds { &self.world.kinds }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct SysCount { pub invocations: u64, pub proposals: u64, pub applied: u64, pub rejected: u64, pub noop: u64, pub schedules: u64 }
#[derive(Clone, Debug, Default, Serialize)]
pub struct RuleCount { pub fired: u64, pub rejected: u64 }
#[derive(Clone, Debug, Default, Serialize)]
pub struct Crossing { pub scheduled: u64, pub fired: u64, pub refused: u64, pub cancelled: u64, pub dropped: u64 }
#[derive(Clone, Debug, Serialize)]
pub struct BoundedWatch { pub min_seen: f64, pub max_seen: f64, pub at_bound_since: Option<SimTime>, pub samples: u64, pub at_min_hits: u64, pub at_max_hits: u64 }
impl Default for BoundedWatch {
    fn default() -> Self { BoundedWatch { min_seen: f64::INFINITY, max_seen: f64::NEG_INFINITY, at_bound_since: None, samples: 0, at_min_hits: 0, at_max_hits: 0 } }
}
#[derive(Clone, Debug, Default, Serialize)]
pub struct InvCount { pub armed: u64, pub satisfied: u64, pub violated: u64 }

/// Incremented where the engine already is. The dynamic half of the harness.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Counters {
    pub events: u64,
    pub applied: u64,
    pub noop: u64,
    pub rejected: u64,
    pub dropped_dead: u64,
    pub same_instant_drops: u64,
    pub undeclared_schedules: u64,
    pub unknown_schedules: u64,
    pub past_schedules: u64,
    pub per_system: Vec<SysCount>,
    pub per_rule: Vec<RuleCount>,
    pub crossings: IndexMap<String, Crossing>,
    pub bounded: Vec<BoundedWatch>,
    pub invariants: Vec<InvCount>,
    pub events_by_kind: IndexMap<String, u64>,
    /// invocations per (system id, event kind id): the loop-brake watch
    pub per_system_event: IndexMap<(u16, u16), u64>,
}

#[derive(Clone, Debug)]
pub struct StepReport { pub event: EventId, pub at: SimTime, pub kind: &'static str, pub subject: EntityId, pub applied: u32, pub rejected: u32, pub scheduled: u32, pub dropped: bool }
#[derive(Clone, Debug)]
pub struct RunReport { pub steps: u64, pub applied: u64, pub rejected: u64, pub from: SimTime, pub to: SimTime, pub hash: [u8; 32] }

pub struct Engine {
    pub world: World,
    pub fel: Fel,
    pub systems: Vec<ResolvedSystem>,
    dispatch: Vec<Vec<usize>>,
    rules: Vec<Arc<dyn Rule>>,
    field_rules: IndexMap<(ComponentKind, FieldId), Vec<usize>>,
    sig_rules: Vec<usize>,
    pub bounded: Vec<ResolvedBounded>,
    pub invariants: Vec<ResolvedInvariant>,
    pub loops: Vec<crate::registry::LoopDecl>,
    pub counters: Counters,
    pub log: LogWriter,
    pub seed: u64,
    pub checkpoint_every: u64,
    externals: Vec<EventKind>,
    k_invariant: EventKind,
    k_law: EventKind,
    instant: IndexMap<EventId, u32>,
    pub plugins: Vec<&'static str>,
    pub content_hash: [u8; 32],
    pub last_checkpoint: Option<[u8; 32]>,
}

fn reject(rule: &str, entity: EntityId, component: Option<&str>, field: Option<&str>, old: Value, attempted: Value, reason: impl Into<String>) -> Verdict {
    Verdict::Rejected(Violation { rule: rule.into(), entity, component: component.map(|s| s.into()), field: field.map(|s| s.into()), old, attempted, reason: reason.into() })
}

impl Engine {
    pub(crate) fn from_blueprint(bp: &Blueprint, seed: u64, log: LogWriter, checkpoint_every: u64) -> Result<Engine, String> { Self::from_blueprint_with(bp, seed, log, checkpoint_every, false) }
    /// THE EPOCH's engine: the blueprint's kinds, rules and systems with NO scene loaded and NO `init` scheduled; the
    /// state comes whole from the epoch record.
    pub(crate) fn from_blueprint_bare(bp: &Blueprint, seed: u64, log: LogWriter, checkpoint_every: u64) -> Result<Engine, String> { Self::from_blueprint_with(bp, seed, log, checkpoint_every, true) }
    fn from_blueprint_with(bp: &Blueprint, seed: u64, mut log: LogWriter, checkpoint_every: u64, bare: bool) -> Result<Engine, String> {
        let stores = bp.components.iter().map(|c| (c.ctor)()).collect();
        let mut world = World::new(bp.kinds.clone(), stores, bp.constants.clone(), bp.protos.clone(),
            bp.systems.iter().map(|s| s.reads).collect(), bp.systems.iter().map(|s| s.desc.consts.iter().chain(s.desc.consts_opt.iter()).copied().collect()).collect());
        let w = world.entities.alloc();
        debug_assert_eq!(w, EntityId::WORLD);
        let lk = world.kind_of::<Label>().unwrap();
        world.store_mut(lk).insert_any(w, Box::new(Label { name: "world".into() }), SimTime::ZERO)?;
        world.set_signature(w, Signature::from_kinds([lk]));
        world.names.insert("world".into(), w);
        log.rule_versions = bp.rule_versions;
        if !bare { log.append(Record::Header { seed, schema_version: crate::log::SCHEMA_VERSION, checkpoint_every, created: String::new(), content_hash: bp.content_hash, plugins: bp.plugins.iter().map(|p| p.to_string()).collect() }); }

        let mut dispatch: Vec<Vec<usize>> = vec![Vec::new(); bp.kinds.events.len()];
        for (i, s) in bp.systems.iter().enumerate() { for k in &s.handles { dispatch[k.0 as usize].push(i); } }
        let mut field_rules: IndexMap<(ComponentKind, FieldId), Vec<usize>> = IndexMap::new();
        let mut sig_rules = Vec::new();
        for (i, r) in bp.rules.iter().enumerate() {
            if r.touches().is_empty() { sig_rules.push(i); }
            for t in r.touches() { field_rules.entry(*t).or_default().push(i); }
        }
        let counters = Counters {
            per_system: vec![SysCount::default(); bp.systems.len()],
            per_rule: vec![RuleCount::default(); bp.rules.len()],
            bounded: vec![BoundedWatch::default(); bp.bounded.len()],
            invariants: vec![InvCount::default(); bp.invariants.len()],
            ..Default::default()
        };
        let k_init = bp.kinds.event(EV_INIT).expect("init");
        let k_invariant = bp.kinds.event(EV_INVARIANT).expect("invariant");
        let k_law = bp.kinds.event(EV_LAW).expect("law");
        let mut engine = Engine {
            world, fel: Fel::new(), systems: bp.systems.clone(), dispatch, rules: bp.rules.clone(), field_rules, sig_rules,
            bounded: bp.bounded.clone(), invariants: bp.invariants.clone(), loops: bp.loops.clone(), counters, log, seed, checkpoint_every: checkpoint_every.max(1),
            externals: bp.externals.clone(), k_invariant, k_law, instant: IndexMap::new(), plugins: bp.plugins.clone(), content_hash: bp.content_hash,
            last_checkpoint: None,
        };
        if !bare {
            for scene in &bp.scenes { engine.load_scene(scene)?; }
            engine.fel.push(Schedule::new(SimTime::ZERO, EV_INIT, EntityId::WORLD, "engine start").prio(PRIO_FIRST), k_init, None);
        }
        Ok(engine)
    }

    /// Content goes through the same rules as everything else: an object that cannot follow the
    /// rules cannot be generated.
    fn load_scene(&mut self, scene: &LoadedScene) -> Result<(), String> {
        let mut ids: IndexMap<String, EntityId> = IndexMap::new();
        for spec in &scene.entities {
            if spec.name == "world" {
                // resources: components on the world entity (ARCHITECTURE 28)
                for c in &spec.components {
                    let mut committed = Vec::new();
                    if let Verdict::Rejected(v) = self.apply_change(EntityId::WORLD, Change::Insert(c.clone()), None, EventId::GENESIS, &[], &mut committed) {
                        return Err(format!("world resource `{}` refused: {v}", c.kind_name()));
                    }
                }
                ids.insert("world".into(), EntityId::WORLD);
                continue;
            }
            // proto components first, then the entity's own (an override replaces the whole component)
            let mut boxed: Vec<Box<dyn AnyComponent>> = Vec::new();
            if let Some(p) = &spec.proto {
                let proto = self.world.protos.get(p).ok_or_else(|| format!("unknown proto {p}"))?;
                boxed.extend(proto.components.iter().cloned());
            }
            for c in &spec.components {
                boxed.retain(|b| b.kind_name() != c.kind_name());
                boxed.push(c.clone());
            }
            // scenes compose by name: a later scene that names an entity an earlier one created adds
            // its components to that entity (a later content file can add components to an entity an earlier one created)
            if let Some(existing) = self.world.find(&spec.name) {
                if spec.proto.is_some() { return Err(format!("scene entity `{}` already exists; a proto cannot be re-applied", spec.name)); }
                for c in &spec.components {
                    let mut committed = Vec::new();
                    if let Verdict::Rejected(v) = self.apply_change(existing, Change::Insert(c.clone()), None, EventId::GENESIS, &[], &mut committed) {
                        return Err(format!("scene entity `{}`: adding `{}` refused: {v}", spec.name, c.kind_name()));
                    }
                }
                ids.insert(spec.name.clone(), existing);
                continue;
            }
            if !boxed.iter().any(|b| b.kind_name() == Label::NAME) { boxed.push(Box::new(Label { name: spec.name.clone() })); }
            let e = self.world.entities.reserve();
            let mut committed = Vec::new();
            match self.apply_change(e, Change::Spawn { components: boxed }, None, EventId::GENESIS, &[e], &mut committed) {
                Verdict::Applied | Verdict::Noop => {}
                Verdict::Rejected(v) => { self.world.entities.release(e); return Err(format!("scene entity `{}` refused: {v}", spec.name)); }
            }
            ids.insert(spec.name.clone(), e);
        }
        for spec in &scene.entities {
            let src = ids[&spec.name];
            for ed in &spec.edges {
                let dst = ids.get(&ed.dst).copied().or_else(|| self.world.find(&ed.dst)).ok_or_else(|| format!("entity `{}` edge to unknown `{}`", spec.name, ed.dst))?;
                let kind = self.world.kinds.relation_static(&ed.kind).ok_or_else(|| format!("unknown relation {}", ed.kind))?;
                let mut committed = Vec::new();
                if let Verdict::Rejected(v) = self.apply_change(src, Change::Edge(EdgeOp::Open { kind, dst, strength: None }), None, EventId::GENESIS, &[], &mut committed) {
                    return Err(format!("scene edge {} -{}-> {} refused: {v}", spec.name, ed.kind, ed.dst));
                }
            }
        }
        Ok(())
    }

    pub fn now(&self) -> SimTime { self.world.now }
    pub fn kinds(&self) -> &Kinds { &self.world.kinds }
    pub fn rules(&self) -> &[Arc<dyn Rule>] { &self.rules }
    pub fn is_external(&self, k: EventKind) -> bool { self.externals.contains(&k) }

    /// Pop one event and process it. `None` when the FEL is empty.
    pub fn step(&mut self) -> Option<StepReport> {
        let ev = self.fel.pop()?;
        self.counters.events += 1;
        *self.counters.events_by_kind.entry(ev.kind_name.to_string()).or_insert(0) += 1;
        if ev.at > self.world.now { self.world.now = ev.at; self.instant.clear(); }
        let mut rep = StepReport { event: ev.id, at: ev.at, kind: ev.kind_name, subject: ev.subject, applied: 0, rejected: 0, scheduled: 0, dropped: false };
        let depth = ev.cause.and_then(|c| self.instant.get(&c).copied()).map_or(0, |d| d + 1);
        if depth > MAX_SAME_INSTANT_CHAIN {
            self.counters.same_instant_drops += 1;
            self.log.append(Record::Defect { event: Some(ev.id), text: format!("same-instant chain deeper than {MAX_SAME_INSTANT_CHAIN} at {} ({}): dropped - LAW 7, a loop with no delay", ev.at, ev.kind_name) });
            rep.dropped = true;
            return Some(rep);
        }
        if self.instant.len() > MAX_SAME_INSTANT_EVENTS {
            self.counters.same_instant_drops += 1;
            self.log.append(Record::Defect { event: Some(ev.id), text: format!("more than {MAX_SAME_INSTANT_EVENTS} events at {} ({}): flood - dropped", ev.at, ev.kind_name) });
            rep.dropped = true;
            return Some(rep);
        }
        self.instant.insert(ev.id, depth);
        if !self.world.alive(ev.subject) {
            self.counters.dropped_dead += 1;
            if !ev.why.is_empty() { self.counters.crossings.entry(ev.why.to_string()).or_default().dropped += 1; }
            self.log.append(Record::Dropped { id: ev.id, at: ev.at, kind: ev.kind_name.into(), subject: ev.subject, reason: "subject no longer alive".into() });
            rep.dropped = true;
            return Some(rep);
        }
        self.log.append(Record::Event { id: ev.id, at: ev.at, kind: ev.kind_name.into(), subject: ev.subject, cause: ev.cause, why: ev.why.into() });
        if !ev.payload.0.is_empty() { self.log.append(Record::EventPayload { id: ev.id, data: ev.payload.0.clone() }); }
        if !ev.why.is_empty() { self.counters.crossings.entry(ev.why.to_string()).or_default().fired += 1; }
        if ev.kind == self.k_invariant {
            self.check_invariant(&ev);
            return Some(rep);
        }
        // A LAW IS AMENDED THROUGH THE ONLY DOOR (2026-09-01): the External rewrites one constant, or flips one
        // switch, from this event on; the log holds it, so a replay lands it at the same event and the state
        // after it is the same state. No system handles it, no rule, no constant row: the content hash is untouched.
        if ev.kind == self.k_law {
            if let Some(l) = ev.decode::<LawSet>() {
                let before = self.world.k_opt(&l.name);
                self.world.laws.insert(l.name.clone(), l.value);
                self.log.append(Record::Note { event: ev.id, system: "core".into(), text: format!("law {} = {} (before: {})", l.name, l.value, before.map_or("unset".to_string(), |v| v.to_string())) });
            } else {
                self.log.append(Record::Defect { event: Some(ev.id), text: "law_set with a payload that is not a LawSet".into() });
            }
            return Some(rep);
        }
        let handlers = self.dispatch[ev.kind.0 as usize].clone();
        for si in handlers { self.run_handler_into(si, &ev, &mut rep); }
        if self.counters.events % self.checkpoint_every == 0 { self.checkpoint(); }
        Some(rep)
    }

    /// One system's handler on one event, its proposals applied (the body of `step`; also the epoch's founding init).
    pub(crate) fn run_handler(&mut self, si: usize, ev: &Event) -> StepReport {
        let mut rep = StepReport { event: ev.id, at: ev.at, kind: ev.kind_name, subject: ev.subject, applied: 0, rejected: 0, scheduled: 0, dropped: false };
        self.run_handler_into(si, ev, &mut rep);
        rep
    }
    fn run_handler_into(&mut self, si: usize, ev: &Event, rep: &mut StepReport) {
        self.counters.per_system[si].invocations += 1;
        *self.counters.per_system_event.entry((si as u16, ev.kind.0)).or_insert(0) += 1;
        self.world.current_system.set(Some(si as u16));
        let (out, reserved) = {
            let mut ctx = Ctx::new(&self.world, self.seed, SystemId(si as u16), ev.id);
            let out = (self.systems[si].desc.handler)(&self.world, ev, &mut ctx);
            (out, std::mem::take(&mut ctx.reserved))
        };
        self.world.current_system.set(None);
        self.apply(out, si, ev, reserved, rep);
    }

    pub fn run_until(&mut self, t: SimTime) -> RunReport {
        let from = self.world.now;
        let (mut steps, a0, r0) = (0u64, self.counters.applied, self.counters.rejected);
        while let Some(next) = self.fel.peek_time() {
            if next > t { break; }
            if self.step().is_none() { break; }
            steps += 1;
        }
        RunReport { steps, applied: self.counters.applied - a0, rejected: self.counters.rejected - r0, from, to: self.world.now, hash: self.hash() }
    }
    pub fn run_steps(&mut self, n: u64) -> Vec<StepReport> {
        let mut v = Vec::new();
        for _ in 0..n { match self.step() { Some(r) => v.push(r), None => break } }
        v
    }

    /// The ONLY door for the outside world. Logged first, always, so replay reproduces it.
    pub fn inject(&mut self, kind: &str, subject: EntityId, payload: Payload, at: Option<SimTime>) -> Result<EventId, String> {
        let k = self.world.kinds.event(kind).ok_or_else(|| format!("unknown event kind {kind}"))?;
        if !self.externals.contains(&k) { return Err(format!("event kind `{kind}` is not registered as external")); }
        let kind_s = self.world.kinds.event_static(kind).unwrap();
        let at = at.map_or(self.world.now, |t| t.max(self.world.now));
        let after_events = self.counters.events;
        let h = self.fel.push(Schedule::new(at, kind_s, subject, "external").prio(PRIO_FIRST).with_payload(payload.clone()), k, None);
        self.log.append(Record::External { id: h.id(), after_events, now: self.world.now, at, kind: kind.into(), subject, payload: payload.0 });
        Ok(h.id())
    }

    /// Canonical-order blake3 of the whole state.
    pub fn hash(&self) -> [u8; 32] {
        let mut h = blake3::Hasher::new();
        h.update(&self.world.now.0.to_le_bytes());
        self.world.entities.hash_into(&mut h);
        for s in &self.world.stores { s.hash_into(&mut h); }
        self.world.edges.hash_into(&mut h);
        self.fel.hash_into(&mut h);
        // only once a law exists: a world without laws hashes as it always did, so every old checkpoint still matches
        if !self.world.laws.is_empty() {
            h.update(b"laws");
            for (k, v) in &self.world.laws { h.update(k.as_bytes()); h.update(&v.to_le_bytes()); }
        }
        *h.finalize().as_bytes()
    }
    pub fn checkpoint(&mut self) -> [u8; 32] {
        let hash = self.hash();
        self.log.append(Record::Checkpoint { at: self.world.now, events: self.counters.events, hash });
        self.last_checkpoint = Some(hash);
        hash
    }

    /// Replay from a log's `External` records, asserting every `Checkpoint`. Returns (checked, mismatches).
    pub fn replay(bp: &Blueprint, records: &[Envelope], log: LogWriter) -> Result<ReplayReport, String> {
        let Some(Record::Header { seed, checkpoint_every, content_hash, .. }) = records.first().map(|e| &e.record) else { return Err("log has no header".into()) };
        let mut rep = ReplayReport { content_hash_matches: *content_hash == bp.content_hash, ..Default::default() };
        // THE FRESH EPOCH: a log whose second record is the whole state restores it and folds from there.
        // Under a foreign content hash the restore is still attempted (the report says so; `bodysim epoch` is the one
        // caller that goes on, `serve` and `load` refuse), so the arithmetic of the marks below is the same either way.
        let epoch = crate::epoch::epoch_of(records)?;
        let mut engine = match &epoch {
            Some(st) => { rep.epoch = true; Engine::from_epoch(bp, *seed, log, *checkpoint_every, st)?.0 }
            None => bp.build(*seed, log, *checkpoint_every)?,
        };
        // Externals and checkpoints are replayed IN LOG ORDER: a checkpoint written before an
        // injection at the same event count must be compared before the injection lands in the
        // FEL (the FEL is part of the hash). Found by the first `bodysim social` after a checkpoint.
        let marks: Vec<&Record> = records.iter().map(|e| &e.record).filter(|r| matches!(r, Record::External { .. } | Record::Checkpoint { .. })).collect();
        'marks: for m in marks {
            let target = match m { Record::External { after_events, .. } => *after_events, Record::Checkpoint { events, .. } => *events, _ => unreachable!() };
            if target < engine.counters.events {
                if matches!(m, Record::Checkpoint { .. }) { rep.skipped += 1; continue; }
                return Err(format!("external record at {target} events but the replay is already at {}", engine.counters.events));
            }
            while engine.counters.events < target {
                // nothing left to run and the log goes on: this is not the run the log recorded
                if engine.step().is_none() { rep.short_of = Some(target); break 'marks; }
            }
            match m {
                Record::External { at, kind, subject, payload, .. } => { engine.inject(kind, *subject, Payload(payload.clone()), Some(*at))?; }
                Record::Checkpoint { events, hash, at } => {
                    rep.checked += 1;
                    if engine.hash() != *hash { rep.mismatches.push((*events, *at)); }
                }
                _ => unreachable!(),
            }
        }
        rep.final_events = engine.counters.events;
        rep.final_hash = engine.hash();
        rep.engine = Some(engine);
        Ok(rep)
    }

    // ------------------------------------------------------------------ apply

    fn apply(&mut self, out: Out, si: usize, ev: &Event, reserved: Vec<EntityId>, rep: &mut StepReport) {
        let mut cancelled: Vec<&'static str> = Vec::new();
        for h in out.cancels { if let Some(why) = self.fel.cancel(h) { cancelled.push(why); } }
        for (e, kn) in out.cancels_for {
            if let Some(k) = self.world.kinds.event(kn) { cancelled.extend(self.fel.cancel_for(e, Some(k))); }
        }
        for (e, kn, why) in out.cancels_for_why {
            if let Some(k) = self.world.kinds.event(kn) { cancelled.extend(self.fel.cancel_for_why(e, k, why)); }
        }
        for why in cancelled { if !why.is_empty() { self.counters.crossings.entry(why.to_string()).or_default().cancelled += 1; } }
        let mut committed: Vec<EntityId> = Vec::new();
        for (entity, change) in out.changes {
            self.counters.per_system[si].proposals += 1;
            match self.apply_change(entity, change, Some(si), ev.id, &reserved, &mut committed) {
                Verdict::Applied => { self.counters.applied += 1; self.counters.per_system[si].applied += 1; rep.applied += 1; }
                Verdict::Noop => { self.counters.noop += 1; self.counters.per_system[si].noop += 1; }
                Verdict::Rejected(v) => {
                    self.counters.rejected += 1;
                    self.counters.per_system[si].rejected += 1;
                    rep.rejected += 1;
                    self.log.append(Record::Rejected { event: ev.id, by: self.systems[si].desc.name.into(), violation: v });
                }
            }
        }
        for e in reserved { if !committed.contains(&e) { self.world.entities.release(e); } }
        for s in out.schedules {
            let Some(k) = self.world.kinds.event(s.kind) else {
                self.counters.unknown_schedules += 1;
                self.log.append(Record::Defect { event: Some(ev.id), text: format!("system {} scheduled unknown event kind `{}`", self.systems[si].desc.name, s.kind) });
                continue;
            };
            if !self.systems[si].schedules.contains(&k) {
                self.counters.undeclared_schedules += 1;
                self.log.append(Record::Defect { event: Some(ev.id), text: format!("system {} scheduled `{}` without declaring it - refused", self.systems[si].desc.name, s.kind) });
                continue;
            }
            if s.at < self.world.now {
                self.counters.past_schedules += 1;
                self.log.append(Record::Defect { event: Some(ev.id), text: format!("system {} scheduled `{}` in the past ({} < {}) - refused", self.systems[si].desc.name, s.kind, s.at, self.world.now) });
                continue;
            }
            if !s.why.is_empty() { self.counters.crossings.entry(s.why.to_string()).or_default().scheduled += 1; }
            self.fel.push(s, k, Some(ev.id));
            self.counters.per_system[si].schedules += 1;
            rep.scheduled += 1;
        }
        for n in out.notes {
            match n {
                Note::Unreachable { why, threshold, x_inf } => {
                    self.counters.crossings.entry(why.to_string()).or_default().refused += 1;
                    self.log.append(Record::Unreachable { event: ev.id, system: self.systems[si].desc.name.into(), why: why.into(), threshold, x_inf });
                }
                Note::Info(text) => self.log.append(Record::Note { event: ev.id, system: self.systems[si].desc.name.into(), text }),
            }
        }
    }

    fn diff(old: &dyn AnyComponent, new: &dyn AnyComponent) -> Vec<FieldChange> {
        let n = new.field_names().len();
        (0..n).filter_map(|i| {
            let f = FieldId(i as u16);
            let (o, nv) = (old.field(f).unwrap_or(Value::Unit), new.field(f).unwrap_or(Value::Unit));
            if o != nv { Some(FieldChange { field: f, old: o, new: nv }) } else { None }
        }).collect()
    }

    #[allow(clippy::result_large_err)]
    fn check_rules(&mut self, view: &RuleView) -> Result<(), Violation> {
        let mut cands: Vec<usize> = Vec::new();
        if view.op != RuleOp::Set { cands.extend(self.sig_rules.iter().copied()); }
        if let Some(k) = view.component {
            for ch in view.changed { if let Some(v) = self.field_rules.get(&(k, ch.field)) { cands.extend(v.iter().copied()); } }
        }
        cands.sort_unstable();
        cands.dedup();
        for ri in cands {
            let rule = &self.rules[ri];
            if !rule.signature().is_subset(&view.sig_after) { continue; }
            self.counters.per_rule[ri].fired += 1;
            if let Err(reason) = rule.check(&self.world, view) {
                self.counters.per_rule[ri].rejected += 1;
                let (field, old, attempted) = match rule.touches().first() {
                    Some((_, f)) => match view.changed.iter().find(|c| c.field == *f) {
                        Some(c) => (Some(self.world.kinds.field_name(view.component.unwrap(), c.field).to_string()), c.old.clone(), c.new.clone()),
                        None => (None, Value::Unit, Value::Unit),
                    },
                    None => (None, Value::Unit, Value::Unit),
                };
                return Err(Violation {
                    rule: rule.name().into(), entity: view.entity,
                    component: view.component.map(|k| self.world.kinds.component_name(k).to_string()),
                    field, old, attempted, reason,
                });
            }
        }
        Ok(())
    }

    pub(crate) fn apply_change(&mut self, entity: EntityId, change: Change, si: Option<usize>, cause: EventId, reserved: &[EntityId], committed: &mut Vec<EntityId>) -> Verdict {
        let by = si.map_or(SystemId::ENGINE, |i| SystemId(i as u16));
        let by_name: String = si.map_or("engine".into(), |i| self.systems[i].desc.name.into());
        let now = self.world.now;
        match change {
            Change::Set(c) => {
                let kn = c.kind_name();
                let Some(kind) = self.world.kinds.component(kn) else { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "unknown component kind") };
                if !self.world.alive(entity) { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "entity not alive"); }
                let changed = {
                    let Some(old) = self.world.store(kind).get_any(entity) else { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "component absent (use Insert)") };
                    Self::diff(old, &*c)
                };
                if changed.is_empty() { return Verdict::Noop; }
                for ch in &changed {
                    if !ch.new.is_finite() {
                        return reject("finite", entity, Some(kn), Some(self.world.kinds.field_name(kind, ch.field)), ch.old.clone(), ch.new.clone(), "non-finite number");
                    }
                    if let Some(i) = si {
                        if !self.systems[i].may_write(kind, ch.field) {
                            return reject("ownership", entity, Some(kn), Some(self.world.kinds.field_name(kind, ch.field)), ch.old.clone(), ch.new.clone(),
                                format!("system {} does not own {}.{} (LAW 6)", by_name, kn, self.world.kinds.field_name(kind, ch.field)));
                        }
                    }
                }
                let sig = self.world.signature(entity);
                let view = RuleView { entity, op: RuleOp::Set, component: Some(kind), changed: &changed, new: Some(&*c), sig_before: sig, sig_after: sig, by };
                if let Err(v) = self.check_rules(&view) { return Verdict::Rejected(v); }
                if let Err(e) = self.world.store_mut(kind).insert_any(entity, c, now) { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, e); }
                self.after_fields(entity, kind, &changed, cause);
                let fields = changed.iter().map(|ch| (self.world.kinds.field_name(kind, ch.field).to_string(), ch.old.clone(), ch.new.clone())).collect();
                self.log.append(Record::Applied { event: cause, by: by_name, entity, change: ChangeRecord::Set { kind: kn.into(), fields } });
                Verdict::Applied
            }
            Change::Insert(c) => {
                let kn = c.kind_name();
                let Some(kind) = self.world.kinds.component(kn) else { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "unknown component kind") };
                if !self.world.alive(entity) { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "entity not alive"); }
                if self.world.store(kind).has(entity) { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "component already present (use Set)"); }
                if let Some(i) = si {
                    if !self.systems[i].may_structure(kind) {
                        return reject("ownership", entity, Some(kn), None, Value::Unit, Value::Unit, format!("system {by_name} does not own {kn} (LAW 6)"));
                    }
                }
                let changed: Vec<FieldChange> = (0..c.field_names().len()).map(|i| FieldChange { field: FieldId(i as u16), old: Value::Unit, new: c.field(FieldId(i as u16)).unwrap_or(Value::Unit) }).collect();
                for ch in &changed {
                    if !ch.new.is_finite() { return reject("finite", entity, Some(kn), Some(self.world.kinds.field_name(kind, ch.field)), Value::Unit, ch.new.clone(), "non-finite number"); }
                }
                let sig = self.world.signature(entity);
                let view = RuleView { entity, op: RuleOp::Insert, component: Some(kind), changed: &changed, new: Some(&*c), sig_before: sig, sig_after: sig.with(kind), by };
                if let Err(v) = self.check_rules(&view) { return Verdict::Rejected(v); }
                self.world.note_label(entity, &*c);
                if let Err(e) = self.world.store_mut(kind).insert_any(entity, c, now) { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, e); }
                self.world.set_signature(entity, sig.with(kind));
                self.after_fields(entity, kind, &changed, cause);
                let fields = changed.iter().map(|ch| (self.world.kinds.field_name(kind, ch.field).to_string(), ch.new.clone())).collect();
                self.log.append(Record::Applied { event: cause, by: by_name, entity, change: ChangeRecord::Insert { kind: kn.into(), fields } });
                Verdict::Applied
            }
            Change::Remove { kind: kn } => {
                let Some(kind) = self.world.kinds.component(kn) else { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "unknown component kind") };
                if !self.world.alive(entity) { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "entity not alive"); }
                if !self.world.store(kind).has(entity) { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "component absent"); }
                if let Some(i) = si {
                    if !self.systems[i].may_structure(kind) { return reject("ownership", entity, Some(kn), None, Value::Unit, Value::Unit, format!("system {by_name} does not own {kn} (LAW 6)")); }
                }
                let sig = self.world.signature(entity);
                let view = RuleView { entity, op: RuleOp::Remove, component: Some(kind), changed: &[], new: None, sig_before: sig, sig_after: sig.without(kind), by };
                if let Err(v) = self.check_rules(&view) { return Verdict::Rejected(v); }
                self.world.store_mut(kind).remove_any(entity);
                self.world.set_signature(entity, sig.without(kind));
                if kind == self.world.label_kind { self.world.forget_labels_of(entity); }
                self.log.append(Record::Applied { event: cause, by: by_name, entity, change: ChangeRecord::Remove { kind: kn.into() } });
                Verdict::Applied
            }
            Change::Spawn { components } => {
                if !reserved.contains(&entity) || !self.world.entities.reserved(entity) {
                    return reject("engine", entity, None, None, Value::Unit, Value::Unit, "spawn target was not reserved by this handler (use Ctx::reserve)");
                }
                let mut sig = Signature::default();
                let mut kinds_named = Vec::new();
                let mut per_comp: Vec<(ComponentKind, Vec<FieldChange>)> = Vec::new();
                for c in &components {
                    let kn = c.kind_name();
                    let Some(kind) = self.world.kinds.component(kn) else { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "unknown component kind") };
                    if sig.contains(kind) { return reject("engine", entity, Some(kn), None, Value::Unit, Value::Unit, "duplicate component in spawn"); }
                    if let Some(i) = si {
                        if !self.systems[i].may_spawn(kind) { return reject("ownership", entity, Some(kn), None, Value::Unit, Value::Unit, format!("system {by_name} does not declare spawning {kn}")); }
                    }
                    let changed: Vec<FieldChange> = (0..c.field_names().len()).map(|i| FieldChange { field: FieldId(i as u16), old: Value::Unit, new: c.field(FieldId(i as u16)).unwrap_or(Value::Unit) }).collect();
                    for ch in &changed {
                        if !ch.new.is_finite() { return reject("finite", entity, Some(kn), Some(self.world.kinds.field_name(kind, ch.field)), Value::Unit, ch.new.clone(), "non-finite number"); }
                    }
                    sig.insert(kind);
                    kinds_named.push(kn.to_string());
                    per_comp.push((kind, changed));
                }
                for (i, (kind, changed)) in per_comp.iter().enumerate() {
                    let view = RuleView { entity, op: RuleOp::Spawn, component: Some(*kind), changed, new: Some(&*components[i]), sig_before: Signature::default(), sig_after: sig, by };
                    if let Err(v) = self.check_rules(&view) { return Verdict::Rejected(v); }
                }
                self.world.entities.commit(entity);
                committed.push(entity);
                self.world.set_signature(entity, sig);
                for (i, c) in components.into_iter().enumerate() {
                    let kind = per_comp[i].0;
                    self.world.note_label(entity, &*c);
                    self.world.store_mut(kind).insert_any(entity, c, now).expect("kind verified");
                }
                for (kind, changed) in &per_comp { self.after_fields(entity, *kind, changed, cause); }
                self.log.append(Record::Applied { event: cause, by: by_name, entity, change: ChangeRecord::Spawn { kinds: kinds_named } });
                Verdict::Applied
            }
            Change::Despawn => {
                if !self.world.alive(entity) { return reject("engine", entity, None, None, Value::Unit, Value::Unit, "entity not alive"); }
                if entity == EntityId::WORLD { return reject("engine", entity, None, None, Value::Unit, Value::Unit, "the world entity is never despawned"); }
                if let Some(i) = si {
                    if !self.systems[i].desc.may_despawn { return reject("ownership", entity, None, None, Value::Unit, Value::Unit, format!("system {by_name} does not declare despawning")); }
                }
                let sig = self.world.signature(entity);
                let view = RuleView { entity, op: RuleOp::Despawn, component: None, changed: &[], new: None, sig_before: sig, sig_after: Signature::default(), by };
                if let Err(v) = self.check_rules(&view) { return Verdict::Rejected(v); }
                let touching = self.world.edges.touching(entity);
                for id in &touching { self.world.edges.close_edge(*id, now, None); }
                for k in sig.iter() { self.world.store_mut(k).remove_any(entity); }
                self.world.set_signature(entity, Signature::default());
                self.world.forget_labels_of(entity);
                self.world.entities.free(entity);
                self.log.append(Record::Applied { event: cause, by: by_name, entity, change: ChangeRecord::Despawn { closed_edges: touching.len() } });
                Verdict::Applied
            }
            Change::Edge(op) => match op {
                EdgeOp::Open { kind: kn, dst, strength } => {
                    let Some(kind) = self.world.kinds.relation(kn) else { return reject("engine", entity, None, None, Value::Unit, Value::Unit, format!("unknown relation {kn}")) };
                    if !self.world.alive(entity) || !self.world.alive(dst) { return reject("engine", entity, None, None, Value::Unit, Value::Id(dst), "edge endpoint not alive"); }
                    if let Some(i) = si {
                        if !self.systems[i].may_edge(kind) { return reject("ownership", entity, None, None, Value::Unit, Value::Id(dst), format!("system {by_name} does not declare relation {kn}")); }
                    }
                    if let Some(s) = strength { if !s.x0.is_finite() || !s.x_inf.is_finite() { return reject("finite", entity, None, None, Value::Unit, Value::Relax(s), "non-finite strength"); } }
                    let traits = self.world.edges.traits(kind);
                    if traits.acyclic && self.world.edges.would_cycle(kind, entity, dst) {
                        return reject("acyclic", entity, None, None, Value::Unit, Value::Id(dst), format!("{kn}: {entity} → {dst} would close a cycle"));
                    }
                    if let Some(existing) = self.world.edges.open_between(entity, dst, kind) {
                        return reject("engine", entity, None, None, Value::Id(existing.dst), Value::Id(dst), format!("{kn} edge already open (edge {})", existing.id));
                    }
                    let mut closed = Vec::new();
                    if traits.exclusive {
                        for e in self.world.edges.open(entity, kind).iter().filter(|e| e.src == entity).map(|e| e.id).collect::<Vec<_>>() { closed.push(e); }
                    }
                    let id = self.world.edges.open_edge(kind, entity, dst, strength, now, cause);
                    for c in &closed { self.world.edges.close_edge(*c, now, Some(id)); }
                    self.log.append(Record::Applied { event: cause, by: by_name, entity, change: ChangeRecord::EdgeOpen { id, kind: kn.into(), dst, closed } });
                    Verdict::Applied
                }
                EdgeOp::Close { id } => {
                    let Some(e) = self.world.edges.get(id).cloned() else { return reject("engine", entity, None, None, Value::Unit, Value::Unit, format!("no edge {id}")) };
                    if !e.is_open() { return Verdict::Noop; }
                    if let Some(i) = si {
                        if !self.systems[i].may_edge(e.kind) { return reject("ownership", entity, None, None, Value::Unit, Value::Unit, format!("system {by_name} does not declare relation {}", self.world.kinds.relation_name(e.kind))); }
                    }
                    self.world.edges.close_edge(id, now, None);
                    self.log.append(Record::Applied { event: cause, by: by_name, entity, change: ChangeRecord::EdgeClose { id, superseded_by: None } });
                    Verdict::Applied
                }
                EdgeOp::Reanchor { id, strength } => {
                    let Some(e) = self.world.edges.get(id).cloned() else { return reject("engine", entity, None, None, Value::Unit, Value::Unit, format!("no edge {id}")) };
                    if !e.is_open() { return reject("engine", entity, None, None, Value::Unit, Value::Relax(strength), "edge already closed"); }
                    if let Some(i) = si {
                        if !self.systems[i].may_edge(e.kind) { return reject("ownership", entity, None, None, Value::Unit, Value::Unit, format!("system {by_name} does not declare relation {}", self.world.kinds.relation_name(e.kind))); }
                    }
                    if !strength.x0.is_finite() || !strength.x_inf.is_finite() { return reject("finite", entity, None, None, Value::Unit, Value::Relax(strength), "non-finite strength"); }
                    let nid = self.world.edges.open_edge(e.kind, e.src, e.dst, Some(strength), now, cause);
                    self.world.edges.close_edge(id, now, Some(nid));
                    self.log.append(Record::Applied { event: cause, by: by_name.clone(), entity, change: ChangeRecord::EdgeClose { id, superseded_by: Some(nid) } });
                    self.log.append(Record::Applied { event: cause, by: by_name, entity, change: ChangeRecord::EdgeOpen { id: nid, kind: self.world.kinds.relation_name(e.kind).into(), dst: e.dst, closed: vec![id] } });
                    Verdict::Applied
                }
            },
        }
    }

    /// Bounded watches and invariant arming, after a field change was applied.
    fn after_fields(&mut self, entity: EntityId, kind: ComponentKind, changed: &[FieldChange], cause: EventId) {
        let now = self.world.now;
        for (bi, b) in self.bounded.iter().enumerate() {
            if b.kind != kind { continue; }
            for ch in changed.iter().filter(|c| c.field == b.field) {
                let Some(v) = ch.new.scalar_at(now) else { continue };
                let asym = ch.new.asymptote().unwrap_or(v);
                let w = &mut self.counters.bounded[bi];
                w.samples += 1;
                w.min_seen = w.min_seen.min(v);
                w.max_seen = w.max_seen.max(v);
                let eps = 1e-9 * (b.decl.max - b.decl.min).abs().max(1e-9);
                let at_min = v <= b.decl.min + eps && asym <= b.decl.min + eps && b.decl.rest != crate::registry::RestBound::Min;
                let at_max = v >= b.decl.max - eps && asym >= b.decl.max - eps && b.decl.rest != crate::registry::RestBound::Max;
                if at_min { w.at_min_hits += 1; }
                if at_max { w.at_max_hits += 1; }
                if at_min || at_max { if w.at_bound_since.is_none() { w.at_bound_since = Some(now); } } else { w.at_bound_since = None; }
            }
        }
        let mut arm: Vec<(usize, f64)> = Vec::new();
        for (ii, inv) in self.invariants.iter().enumerate() {
            if inv.kind != kind { continue; }
            for ch in changed.iter().filter(|c| c.field == inv.field) {
                let (o, n) = (ch.old.scalar_at(now).unwrap_or(f64::NEG_INFINITY), ch.new.scalar_at(now).unwrap_or(f64::NEG_INFINITY));
                if o < inv.threshold && n >= inv.threshold { arm.push((ii, inv.threshold)); }
            }
        }
        for (ii, _) in arm {
            let inv = &self.invariants[ii];
            self.counters.invariants[ii].armed += 1;
            let s = Schedule::new(now + inv.within, EV_INVARIANT, entity, inv.name).payload(&(ii as u32));
            self.fel.push(s, self.k_invariant, Some(cause));
        }
    }

    fn check_invariant(&mut self, ev: &Event) {
        let Some(ii) = ev.decode::<u32>().map(|i| i as usize) else { return };
        let Some(inv) = self.invariants.get(ii).cloned() else { return };
        let v = self.world.field(ev.subject, inv.kind, inv.field).and_then(|v| v.scalar_at(self.world.now));
        let ok = v.is_none_or(|x| x < inv.threshold);
        if ok { self.counters.invariants[ii].satisfied += 1; } else { self.counters.invariants[ii].violated += 1; }
        self.log.append(Record::Invariant { name: inv.name.into(), entity: ev.subject, threshold: inv.threshold, deadline: ev.at, value_at_deadline: v.unwrap_or(f64::NAN), ok });
    }
}

#[derive(Default)]
pub struct ReplayReport {
    /// the log opened at an epoch record (the state restored, not re-run)
    pub epoch: bool,
    pub checked: u64,
    pub skipped: u64,
    pub mismatches: Vec<(u64, SimTime)>,
    /// The event count of a checkpoint or injection the replay never reached: its event list ran dry first.
    pub short_of: Option<u64>,
    pub final_events: u64,
    pub final_hash: [u8; 32],
    pub content_hash_matches: bool,
    pub engine: Option<Engine>,
}
impl ReplayReport {
    pub fn ok(&self) -> bool { self.mismatches.is_empty() && self.short_of.is_none() && self.content_hash_matches }
}
