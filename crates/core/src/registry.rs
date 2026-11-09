//! The plugin surface. A plugin registers components, events, relations, rules, systems
//! `{reads, writes, handles, schedules}`, bounded fields (THE RATCHET LAW, declarable), invariants,
//! constants, protos and scenes. `finalize` runs the STATIC checks and REFUSES to start on any defect:
//! the old house's whole "built it, never called it" class, caught before a single event.
use crate::component::{store_ctor, AnyStore, Component, ComponentKind, Signature};
use crate::constants::{ConstStatus, Constants};
use crate::edges::{RelKind, RelTraits};
use crate::engine::{Ctx, Engine};
use crate::event::{Event, EventKind};
use crate::id::{FieldId, SystemId};
use crate::log::LogWriter;
use crate::proposal::Out;
use crate::rule::{DeclRule, Rule};
use crate::time::Span;
use crate::world::{Kinds, Label, World};
use indexmap::{IndexMap, IndexSet};
use std::fmt;
use std::sync::Arc;

pub const EV_INIT: &str = "init";

pub use crate::content::from_ron;
use crate::content::{parse_protos, parse_scene, LoadedProto, LoadedScene};
pub const EV_INVARIANT: &str = "invariant_check";
/// THE LAW-SWITCH (2026-09-01): a constant amended, or a named switch flipped, through the only door.
/// An External, logged before it lands, so a log that holds one replays it at the same event; a log
/// that holds none is untouched. The first engine change since the replay law that needs no rebirth.
pub const EV_LAW: &str = "law_set";

pub trait Plugin {
    fn name(&self) -> &'static str;
    fn register(&self, r: &mut Registry);
}

pub type Handler = fn(&World, &Event, &mut Ctx) -> Out;
pub fn noop_handler(_: &World, _: &Event, _: &mut Ctx) -> Out { Out::new() }

/// What a system declares. Names are resolved to ids at `finalize`.
/// `writes` entries: `"kind"` (may Set any field, Insert, Remove) or `"kind.field"` (may Set that field).
#[derive(Clone)]
pub struct SystemDescriptor {
    pub name: &'static str,
    pub plugin: &'static str,
    pub reads: &'static [&'static str],
    pub writes: &'static [&'static str],
    /// component kinds this system may put on entities it spawns
    pub spawns: &'static [&'static str],
    pub may_despawn: bool,
    /// relation kinds this system may open/close
    pub edges: &'static [&'static str],
    pub handles: &'static [&'static str],
    pub schedules: &'static [&'static str],
    pub consts: &'static [&'static str],
    /// kinds this system uses IF another plugin registered them (absent ⇒ silently skipped). The
    /// ecosystem claim: a plugin slots in without the others being present.
    pub reads_opt: &'static [&'static str],
    /// events this system reacts to IF some plugin registered them
    pub handles_opt: &'static [&'static str],
    /// constants read only along an optional path (another plugin's rows); absent ⇒ fine
    pub consts_opt: &'static [&'static str],
    /// events this system schedules IF some plugin registered them (a content effect on a body that may be absent)
    pub schedules_opt: &'static [&'static str],
    pub handler: Handler,
}
impl Default for SystemDescriptor {
    fn default() -> Self {
        SystemDescriptor { name: "", plugin: "", reads: &[], writes: &[], spawns: &[], may_despawn: false, edges: &[], handles: &[], schedules: &[], consts: &[], reads_opt: &[], handles_opt: &[], consts_opt: &[], schedules_opt: &[], handler: noop_handler }
    }
}

#[derive(Clone)]
pub struct ResolvedSystem {
    pub desc: SystemDescriptor,
    pub id: SystemId,
    pub reads: Signature,
    pub writes_all: Signature,
    pub writes_fields: Vec<(ComponentKind, FieldId)>,
    pub spawns: Signature,
    pub edges: Vec<RelKind>,
    pub handles: Vec<EventKind>,
    pub schedules: Vec<EventKind>,
}
impl ResolvedSystem {
    pub fn may_write(&self, k: ComponentKind, f: FieldId) -> bool { self.writes_all.contains(k) || self.writes_fields.contains(&(k, f)) }
    pub fn may_structure(&self, k: ComponentKind) -> bool { self.writes_all.contains(k) }
    pub fn may_spawn(&self, k: ComponentKind) -> bool { self.spawns.contains(k) }
    pub fn may_edge(&self, r: RelKind) -> bool { self.edges.contains(&r) }
}

/// (system, event) - a named mechanism that raises or lowers a bounded field.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct Mechanism { pub system: &'static str, pub on: &'static str }

/// What stops a feedback loop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Brake {
    /// a registered (system, event): the harness counts whether it fired
    Internal(Mechanism),
    /// braked by construction - a saturating read or a bound, no event needed
    Formula(&'static str),
    /// the literature names no internal brake: an external event must arrive (reported, never required)
    External(&'static str),
    /// the loop's mechanism lives in a backlog plugin that is not built yet
    Backlog(&'static str),
}

/// A feedback loop in the model, declared so the coverage report can
/// list every loop with whether its brake fired.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LoopDecl { pub number: u8, pub name: &'static str, pub vicious: bool, pub brake: Brake, pub source: &'static str }

/// Which bound, if any, is the field's natural resting value (a dry surface at 0, a full charge
/// at 1). Resting there is not "stuck"; the saturation watch only flags the OTHER bound.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Default)]
pub enum RestBound { #[default] Neither, Min, Max }

/// THE RATCHET LAW, made declarable. Both lists MUST be non-empty; `finalize` verifies each
/// mechanism's system writes the field, handles the event, and that the event is schedulable.
#[derive(Clone, Debug)]
pub struct BoundedDecl {
    pub kind: &'static str,
    pub field: &'static str,
    pub min: f64,
    pub max: f64,
    pub raised_by: &'static [Mechanism],
    pub lowered_by: &'static [Mechanism],
    /// for the saturation watch: "stuck" = at a bound for > 3·tau
    pub tau_hint: Option<Span>,
    pub rest: RestBound,
}

#[derive(Clone, Debug)]
pub struct ResolvedBounded { pub decl: BoundedDecl, pub kind: ComponentKind, pub field: FieldId }

/// Scheduled invariants - checked on the same FEL, no temporal-logic library.
#[derive(Clone, Debug)]
pub enum Invariant {
    /// after `field` crosses above `threshold`, it must be back below it within `within`
    BoundedResponse { name: &'static str, kind: &'static str, field: &'static str, threshold: f64, within: Span },
}
#[derive(Clone, Debug)]
pub struct ResolvedInvariant { pub name: &'static str, pub kind: ComponentKind, pub field: FieldId, pub threshold: f64, pub within: Span }

#[derive(Clone)]
pub struct ComponentDecl { pub name: &'static str, pub fields: &'static [&'static str], pub plugin: &'static str, pub ctor: fn() -> Box<dyn AnyStore> }
#[derive(Clone, Debug)]
pub struct EventDecl { pub name: &'static str, pub plugin: &'static str, pub external: bool }
#[derive(Clone, Debug)]
pub struct RelDecl { pub name: &'static str, pub traits: RelTraits, pub plugin: &'static str }

type RuleFactory = Box<dyn Fn(&Kinds) -> Result<Box<dyn Rule>, String>>;

/// Every reason `finalize` refuses to start.
#[derive(Clone, Debug, PartialEq)]
pub enum Defect {
    UnknownComponent { system: String, name: String },
    UnknownField { where_: String, kind: String, field: String },
    UnknownEvent { system: String, name: String },
    UnknownRelation { system: String, name: String },
    DuplicateName { what: String, name: String },
    TooManyKinds { count: usize },
    ComponentUnread { kind: String, plugin: String },
    ComponentUnwritten { kind: String, plugin: String },
    RuleBad { text: String },
    RuleUnbindable { rule: String, kind: String },
    EventUnhandled { event: String },
    EventUnscheduled { event: String, handled_by: Vec<String> },
    BoundedNoRaiser { kind: String, field: String },
    BoundedNoLowerer { kind: String, field: String },
    MechanismInvalid { kind: String, field: String, system: String, on: String, reason: String },
    LowererUnreachable { kind: String, field: String, system: String, on: String },
    DuplicateWriter { kind: String, field: String, systems: Vec<String> },
    ConstantMissing { system: String, name: String },
    ConstantForbidden { system: String, name: String, status: String },
    ProtoUnknownComponent { proto: String, kind: String },
    SceneError { text: String },
}
impl fmt::Display for Defect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Defect::UnknownComponent { system, name } => write!(f, "system `{system}` names unknown component `{name}`"),
            Defect::UnknownField { where_, kind, field } => write!(f, "{where_}: component `{kind}` has no field `{field}`"),
            Defect::UnknownEvent { system, name } => write!(f, "system `{system}` names unknown event `{name}`"),
            Defect::UnknownRelation { system, name } => write!(f, "system `{system}` names unknown relation `{name}`"),
            Defect::DuplicateName { what, name } => write!(f, "duplicate {what} `{name}`"),
            Defect::TooManyKinds { count } => write!(f, "{count} component kinds registered; the signature bitset holds 256"),
            Defect::ComponentUnread { kind, plugin } => write!(f, "component `{kind}` ({plugin}) is read by nobody - dead wire"),
            Defect::ComponentUnwritten { kind, plugin } => write!(f, "component `{kind}` ({plugin}) is written/spawned/loaded by nobody - dead wire"),
            Defect::RuleBad { text } => write!(f, "rule: {text}"),
            Defect::RuleUnbindable { rule, kind } => write!(f, "rule `{rule}` can never bind: nothing ever creates component `{kind}`"),
            Defect::EventUnhandled { event } => write!(f, "event `{event}` is handled by no system"),
            Defect::EventUnscheduled { event, handled_by } => write!(f, "event `{event}` (handled by {handled_by:?}) is scheduled by nobody - the handler can never run"),
            Defect::BoundedNoRaiser { kind, field } => write!(f, "bounded `{kind}.{field}` declares no raiser"),
            Defect::BoundedNoLowerer { kind, field } => write!(f, "bounded `{kind}.{field}` declares no lowerer - THE RATCHET LAW"),
            Defect::MechanismInvalid { kind, field, system, on, reason } => write!(f, "bounded `{kind}.{field}` mechanism {system}@{on}: {reason}"),
            Defect::LowererUnreachable { kind, field, system, on } => write!(f, "bounded `{kind}.{field}` lowerer {system}@{on}: event `{on}` is scheduled by nobody - unreachable lowerer"),
            Defect::DuplicateWriter { kind, field, systems } => write!(f, "field `{kind}.{field}` has {} writers {systems:?} - LAW 6: one owner", systems.len()),
            Defect::ConstantMissing { system, name } => write!(f, "system `{system}` declares constant `{name}` which is not in content"),
            Defect::ConstantForbidden { system, name, status } => write!(f, "system `{system}` reads constant `{name}` marked {status}"),
            Defect::ProtoUnknownComponent { proto, kind } => write!(f, "proto `{proto}` uses unknown component `{kind}`"),
            Defect::SceneError { text } => write!(f, "scene: {text}"),
        }
    }
}

/// The wiring, computed from declarations (the dead-wire report).
#[derive(Clone, Debug, Default)]
pub struct StaticReport {
    pub plugins: Vec<String>,
    /// (kind, plugin, readers, writers)
    pub components: Vec<(String, String, Vec<String>, Vec<String>)>,
    /// (event, handlers, schedulers)
    pub events: Vec<(String, Vec<String>, Vec<String>)>,
    pub relations: Vec<(String, RelTraits)>,
    pub systems: Vec<String>,
    pub rules: Vec<String>,
    pub bounded: Vec<String>,
    pub invariants: Vec<String>,
    pub loops: Vec<String>,
    pub tunables: Vec<String>,
    pub contested: Vec<String>,
    /// optional kinds/events a system would have used but no plugin registered
    pub optional_absent: Vec<String>,
    pub defects: Vec<Defect>,
}
impl StaticReport {
    pub fn markdown(&self) -> String {
        let mut s = String::new();
        s.push_str("# Static report - the wiring\n\n");
        s.push_str(&format!("plugins: {}\n\n", self.plugins.join(", ")));
        s.push_str("## Components (kind · plugin · readers · writers)\n\n| kind | plugin | readers | writers |\n|---|---|---|---|\n");
        for (k, p, r, w) in &self.components { s.push_str(&format!("| {k} | {p} | {} | {} |\n", r.join(", "), w.join(", "))); }
        s.push_str("\n## Events (handlers · schedulers)\n\n| event | handled by | scheduled by |\n|---|---|---|\n");
        for (e, h, sc) in &self.events { s.push_str(&format!("| {e} | {} | {} |\n", h.join(", "), sc.join(", "))); }
        s.push_str("\n## Relations\n\n");
        for (r, t) in &self.relations { s.push_str(&format!("- {r} {t:?}\n")); }
        s.push_str(&format!("\n## Systems ({})\n\n", self.systems.len()));
        for x in &self.systems { s.push_str(&format!("- {x}\n")); }
        s.push_str(&format!("\n## Rules ({})\n\n", self.rules.len()));
        for x in &self.rules { s.push_str(&format!("- {x}\n")); }
        s.push_str(&format!("\n## Bounded fields ({})\n\n", self.bounded.len()));
        for x in &self.bounded { s.push_str(&format!("- {x}\n")); }
        s.push_str(&format!("\n## Invariants ({})\n\n", self.invariants.len()));
        for x in &self.invariants { s.push_str(&format!("- {x}\n")); }
        s.push_str(&format!("\n## Loops ({})\n\n", self.loops.len()));
        for x in &self.loops { s.push_str(&format!("- {x}\n")); }
        s.push_str(&format!("\n## Tunable constants ({})\n\n", self.tunables.len()));
        for x in &self.tunables { s.push_str(&format!("- {x}\n")); }
        s.push_str(&format!("\n## Contested constants ({})\n\n", self.contested.len()));
        for x in &self.contested { s.push_str(&format!("- {x}\n")); }
        s.push_str(&format!("\n## Optional wiring absent ({})\n\n", self.optional_absent.len()));
        for x in &self.optional_absent { s.push_str(&format!("- {x}\n")); }
        s.push_str(&format!("\n## Defects ({})\n\n", self.defects.len()));
        for d in &self.defects { s.push_str(&format!("- ❌ {d}\n")); }
        s
    }
}

pub struct Registry {
    current: &'static str,
    plugins: Vec<&'static str>,
    components: Vec<ComponentDecl>,
    events: Vec<EventDecl>,
    relations: Vec<RelDecl>,
    systems: Vec<SystemDescriptor>,
    decl_rules: Vec<DeclRule>,
    code_rules: Vec<RuleFactory>,
    bounded: Vec<BoundedDecl>,
    invariants: Vec<Invariant>,
    loops: Vec<LoopDecl>,
    constants: Constants,
    proto_texts: Vec<(&'static str, String)>,
    scene_texts: Vec<(&'static str, String)>,
    errors: Vec<Defect>,
}

impl Default for Registry {
    fn default() -> Self { Self::new() }
}

impl Registry {
    pub fn new() -> Registry {
        let mut r = Registry {
            current: "core", plugins: vec!["core"], components: Vec::new(), events: Vec::new(), relations: Vec::new(), systems: Vec::new(),
            decl_rules: Vec::new(), code_rules: Vec::new(), bounded: Vec::new(), invariants: Vec::new(), loops: Vec::new(), constants: Constants::default(),
            proto_texts: Vec::new(), scene_texts: Vec::new(), errors: Vec::new(),
        };
        r.component::<Label>();
        r.events.push(EventDecl { name: EV_INIT, plugin: "core", external: false });
        r.events.push(EventDecl { name: EV_INVARIANT, plugin: "core", external: false });
        r
    }
    pub fn add(&mut self, p: &dyn Plugin) -> &mut Self {
        self.current = p.name();
        if !self.plugins.contains(&p.name()) { self.plugins.push(p.name()); }
        p.register(self);
        self.current = "core";
        self
    }
    pub fn component<T: Component>(&mut self) -> &mut Self {
        self.components.push(ComponentDecl { name: T::NAME, fields: T::FIELDS, plugin: self.current, ctor: store_ctor::<T> });
        self
    }
    pub fn event(&mut self, name: &'static str) -> &mut Self { self.events.push(EventDecl { name, plugin: self.current, external: false }); self }
    /// An event kind the outside world may inject through `Engine::inject`.
    pub fn external_event(&mut self, name: &'static str) -> &mut Self { self.events.push(EventDecl { name, plugin: self.current, external: true }); self }
    pub fn relation(&mut self, name: &'static str, traits: RelTraits) -> &mut Self { self.relations.push(RelDecl { name, traits, plugin: self.current }); self }
    pub fn system(&mut self, mut d: SystemDescriptor) -> &mut Self {
        if d.plugin.is_empty() { d.plugin = self.current; }
        self.systems.push(d);
        self
    }
    pub fn rule(&mut self, d: DeclRule) -> &mut Self { self.decl_rules.push(d); self }
    pub fn rules_ron(&mut self, text: &str) -> &mut Self {
        match from_ron::<Vec<DeclRule>>(text) {
            Ok(rs) => self.decl_rules.extend(rs),
            Err(e) => self.errors.push(Defect::RuleBad { text: format!("{}: {e}", self.current) }),
        }
        self
    }
    /// A typed rule, built once the kind table exists.
    pub fn rule_with(&mut self, f: impl Fn(&Kinds) -> Result<Box<dyn Rule>, String> + 'static) -> &mut Self { self.code_rules.push(Box::new(f)); self }
    pub fn bounded(&mut self, b: BoundedDecl) -> &mut Self { self.bounded.push(b); self }
    pub fn invariant(&mut self, i: Invariant) -> &mut Self { self.invariants.push(i); self }
    /// Declare a feedback loop and its brake.
    pub fn loop_decl(&mut self, l: LoopDecl) -> &mut Self { self.loops.push(l); self }
    /// A later plugin closes a loop an earlier one declared open (the connection brakes what the
    /// psyche listed as backlog). Unknown loop numbers are a defect at `finalize`.
    pub fn loop_brake(&mut self, number: u8, brake: Brake) -> &mut Self {
        match self.loops.iter_mut().find(|l| l.number == number) {
            Some(l) => l.brake = brake,
            None => self.errors.push(Defect::SceneError { text: format!("loop_brake: loop {number} was never declared") }),
        }
        self
    }
    pub fn constants_ron(&mut self, prefix: &str, text: &str) -> &mut Self {
        if let Err(e) = self.constants.load_ron(prefix, text) { self.errors.push(Defect::SceneError { text: e }); }
        self
    }
    /// Protos: `[ (name: "cup", components: { "kind": (...) }), ... ]`. Parsed, typed, at `finalize`.
    pub fn protos_ron(&mut self, text: &str) -> &mut Self { self.proto_texts.push((self.current, text.to_string())); self }
    /// A scene: `(entities: [ (name: "cup", proto: "cup", components: {...}, edges: [(kind: "in", dst: "kitchen")]) ])`.
    pub fn scene_ron(&mut self, text: &str) -> &mut Self { self.scene_texts.push((self.current, text.to_string())); self }
    pub fn constants(&self) -> &Constants { &self.constants }

    /// Static analysis + resolution. Refuses on any defect.
    pub fn finalize(mut self) -> Result<Blueprint, Vec<Defect>> {
        // THE LAW EVENT IS REGISTERED LAST, after every plugin (2026-09-01): an event kind is its index, the
        // FEL hash carries the index of every pending event, and registering it in `new()` shifted every
        // plugin's kinds by one, so a log written before laws existed mismatched at its first checkpoint.
        // Appended here it takes the index nothing else had, and every old checkpoint still matches.
        if !self.events.iter().any(|e| e.name == EV_LAW) { self.events.push(EventDecl { name: EV_LAW, plugin: "core", external: true }); }
        let (bp, report) = self.resolve();
        match bp {
            Some(bp) if report.defects.is_empty() => Ok(bp),
            _ => Err(report.defects),
        }
    }
    /// The static report even when defective (for `bodysim report deadwires`).
    pub fn analyse(self) -> StaticReport { self.resolve().1 }

    fn resolve(self) -> (Option<Blueprint>, StaticReport) {
        let mut defects = self.errors.clone();
        let mut report = StaticReport { plugins: self.plugins.iter().map(|p| p.to_string()).collect(), ..Default::default() };

        // 1. kind tables in sorted-name order
        let mut comps = self.components.clone();
        comps.sort_by(|a, b| a.name.cmp(b.name));
        let mut kinds = Kinds::default();
        for c in &comps {
            if kinds.components.contains_key(c.name) { defects.push(Defect::DuplicateName { what: "component".into(), name: c.name.into() }); continue; }
            let id = ComponentKind(kinds.components.len() as u16);
            kinds.components.insert(c.name, id);
            kinds.fields.push(c.fields);
        }
        comps.dedup_by(|a, b| a.name == b.name);
        if kinds.components.len() > Signature::MAX_KINDS { defects.push(Defect::TooManyKinds { count: kinds.components.len() }); }
        // THE LAW EVENT TAKES THE LAST INDEX, outside the sorted table (2026-09-01): an event kind is its
        // index and the FEL hash carries the index of every pending event, so a kind sorted into the middle
        // shifts every kind after it and a log written before laws existed mismatches at its first
        // checkpoint. Last, it is the one index nothing else ever had. (A NEW PLUGIN EVENT still shifts
        // the table: that is still a rebirth. A law gates behaviour; it does not add a kind.)
        let mut evs: Vec<EventDecl> = self.events.iter().filter(|e| e.name != EV_LAW).cloned().collect();
        evs.sort_by(|a, b| a.name.cmp(b.name));
        evs.extend(self.events.iter().filter(|e| e.name == EV_LAW).cloned());
        for e in &evs {
            if kinds.events.contains_key(e.name) {
                defects.push(Defect::DuplicateName { what: "event".into(), name: e.name.into() });
                continue;
            }
            kinds.events.insert(e.name, EventKind(kinds.events.len() as u16));
        }
        let mut rels = self.relations.clone();
        rels.sort_by(|a, b| a.name.cmp(b.name));
        for r in &rels {
            if kinds.relations.contains_key(r.name) { defects.push(Defect::DuplicateName { what: "relation".into(), name: r.name.into() }); continue; }
            kinds.relations.insert(r.name, RelKind(kinds.relations.len() as u16));
            kinds.rel_traits.push(r.traits);
        }
        let externals: Vec<EventKind> = evs.iter().filter(|e| e.external).filter_map(|e| kinds.event(e.name)).collect();

        // 2. systems
        let mut systems: Vec<ResolvedSystem> = Vec::new();
        let mut seen_sys: IndexSet<&str> = IndexSet::new();
        for (i, d) in self.systems.iter().enumerate() {
            if !seen_sys.insert(d.name) { defects.push(Defect::DuplicateName { what: "system".into(), name: d.name.into() }); }
            let mut rs = ResolvedSystem { desc: d.clone(), id: SystemId(i as u16), reads: Signature::default(), writes_all: Signature::default(), writes_fields: Vec::new(), spawns: Signature::default(), edges: Vec::new(), handles: Vec::new(), schedules: Vec::new() };
            for n in d.reads { match kinds.component(n) { Some(k) => rs.reads.insert(k), None => defects.push(Defect::UnknownComponent { system: d.name.into(), name: n.to_string() }) } }
            for n in d.reads_opt { if let Some(k) = kinds.component(n) { rs.reads.insert(k); } else { report.optional_absent.push(format!("{}: component `{n}`", d.name)); } }
            for n in d.writes {
                let (kn, fname) = match n.split_once('.') { Some((k, f)) => (k, Some(f)), None => (*n, None) };
                match kinds.component(kn) {
                    Some(k) => match fname {
                        None => rs.writes_all.insert(k),
                        Some(f) => match kinds.field(k, f) { Some(fid) => rs.writes_fields.push((k, fid)), None => defects.push(Defect::UnknownField { where_: format!("system `{}` writes", d.name), kind: kn.into(), field: f.into() }) },
                    },
                    None => defects.push(Defect::UnknownComponent { system: d.name.into(), name: kn.to_string() }),
                }
            }
            for n in d.spawns { match kinds.component(n) { Some(k) => rs.spawns.insert(k), None => defects.push(Defect::UnknownComponent { system: d.name.into(), name: n.to_string() }) } }
            for n in d.edges { match kinds.relation(n) { Some(k) => rs.edges.push(k), None => defects.push(Defect::UnknownRelation { system: d.name.into(), name: n.to_string() }) } }
            for n in d.handles { match kinds.event(n) { Some(k) => rs.handles.push(k), None => defects.push(Defect::UnknownEvent { system: d.name.into(), name: n.to_string() }) } }
            for n in d.handles_opt { if let Some(k) = kinds.event(n) { rs.handles.push(k); } else { report.optional_absent.push(format!("{}: event `{n}`", d.name)); } }
            for n in d.schedules { match kinds.event(n) { Some(k) => rs.schedules.push(k), None => defects.push(Defect::UnknownEvent { system: d.name.into(), name: n.to_string() }) } }
            for n in d.schedules_opt { if let Some(k) = kinds.event(n) { rs.schedules.push(k); } else { report.optional_absent.push(format!("{}: schedules `{n}`", d.name)); } }
            for n in d.consts_opt {
                if let Some(row) = self.constants.get(n) { if row.status == ConstStatus::DoNotBuildOn { defects.push(Defect::ConstantForbidden { system: d.name.into(), name: n.to_string(), status: "DoNotBuildOn".into() }); } }
            }
            for n in d.consts {
                match self.constants.get(n) {
                    None => defects.push(Defect::ConstantMissing { system: d.name.into(), name: n.to_string() }),
                    Some(row) => if row.status == ConstStatus::DoNotBuildOn { defects.push(Defect::ConstantForbidden { system: d.name.into(), name: n.to_string(), status: "DoNotBuildOn".into() }) },
                }
            }
            kinds.systems.insert(d.name, SystemId(i as u16));
            systems.push(rs);
        }
        report.systems = systems.iter().map(|s| format!("{} ({}) handles {:?} schedules {:?}", s.desc.name, s.desc.plugin, s.desc.handles, s.desc.schedules)).collect();

        // 3. one writer per field
        let mut writers: IndexMap<(ComponentKind, FieldId), Vec<usize>> = IndexMap::new();
        for (i, s) in systems.iter().enumerate() {
            for k in s.writes_all.iter() {
                for f in 0..kinds.fields_of(k).len() { writers.entry((k, FieldId(f as u16))).or_default().push(i); }
            }
            for (k, f) in &s.writes_fields { writers.entry((*k, *f)).or_default().push(i); }
        }
        for ((k, f), ws) in &writers {
            let mut names: Vec<String> = ws.iter().map(|i| systems[*i].desc.name.to_string()).collect();
            names.sort();
            names.dedup();
            if names.len() > 1 {
                defects.push(Defect::DuplicateWriter { kind: kinds.component_name(*k).into(), field: kinds.field_name(*k, *f).into(), systems: names });
            }
        }

        // 4. rules
        let mut rules: Vec<Arc<dyn Rule>> = Vec::new();
        for d in &self.decl_rules {
            match d.resolve(&kinds) { Ok(r) => rules.push(Arc::new(r)), Err(e) => defects.push(Defect::RuleBad { text: e }) }
        }
        for f in &self.code_rules {
            match f(&kinds) { Ok(r) => rules.push(Arc::from(r)), Err(e) => defects.push(Defect::RuleBad { text: e }) }
        }

        // 5. bounded → also range rules
        let mut bounded: Vec<ResolvedBounded> = Vec::new();
        for b in &self.bounded {
            let Some(k) = kinds.component(b.kind) else { defects.push(Defect::UnknownComponent { system: "bounded".into(), name: b.kind.into() }); continue };
            let Some(f) = kinds.field(k, b.field) else { defects.push(Defect::UnknownField { where_: "bounded".into(), kind: b.kind.into(), field: b.field.into() }); continue };
            bounded.push(ResolvedBounded { decl: b.clone(), kind: k, field: f });
            match (DeclRule::Range { kind: b.kind.into(), field: b.field.into(), min: b.min, max: b.max }).resolve(&kinds) {
                Ok(r) => rules.push(Arc::new(r)),
                Err(e) => defects.push(Defect::RuleBad { text: e }),
            }
        }
        report.rules = rules.iter().map(|r| format!("{} v{} touches {:?}", r.name(), r.version(), r.touches().iter().map(|(k, f)| format!("{}.{}", kinds.component_name(*k), kinds.field_name(*k, *f))).collect::<Vec<_>>())).collect();

        // 6. invariants
        let mut invariants = Vec::new();
        for i in &self.invariants {
            match i {
                Invariant::BoundedResponse { name, kind, field, threshold, within } => {
                    let Some(k) = kinds.component(kind) else { defects.push(Defect::UnknownComponent { system: format!("invariant {name}"), name: kind.to_string() }); continue };
                    let Some(f) = kinds.field(k, field) else { defects.push(Defect::UnknownField { where_: format!("invariant {name}"), kind: kind.to_string(), field: field.to_string() }); continue };
                    invariants.push(ResolvedInvariant { name, kind: k, field: f, threshold: *threshold, within: *within });
                    report.invariants.push(format!("{name}: {kind}.{field} > {threshold} must fall back within {within}"));
                }
            }
        }

        // 7. protos & scenes: parsed typed, with the kind dispatched by name
        let tmp_stores: Vec<Box<dyn AnyStore>> = comps.iter().map(|c| (c.ctor)()).collect();
        let mut protos: IndexMap<String, LoadedProto> = IndexMap::new();
        for (plugin, text) in &self.proto_texts {
            match parse_protos(text, &tmp_stores, &kinds) {
                Ok(ps) => for p in ps {
                    if protos.contains_key(&p.name) { defects.push(Defect::DuplicateName { what: "proto".into(), name: p.name.clone() }); }
                    protos.insert(p.name.clone(), p);
                },
                Err(e) => defects.push(Defect::SceneError { text: format!("protos ({plugin}): {e}") }),
            }
        }
        let mut scenes: Vec<LoadedScene> = Vec::new();
        for (plugin, text) in &self.scene_texts {
            match parse_scene(text, &tmp_stores, &kinds) {
                Ok(sc) => scenes.push(sc),
                Err(e) => defects.push(Defect::SceneError { text: format!("scene ({plugin}): {e}") }),
            }
        }
        let mut content_kinds: Signature = Signature::default();
        for p in protos.values() { for c in &p.components { if let Some(k) = kinds.component(c.kind_name()) { content_kinds.insert(k); } } }
        for sc in &scenes {
            for e in &sc.entities {
                if let Some(p) = &e.proto { if !protos.contains_key(p) { defects.push(Defect::SceneError { text: format!("entity `{}` uses unknown proto `{p}`", e.name) }); } }
                for c in &e.components { if let Some(k) = kinds.component(c.kind_name()) { content_kinds.insert(k); } }
                for ed in &e.edges { if kinds.relation(&ed.kind).is_none() { defects.push(Defect::SceneError { text: format!("entity `{}` edge uses unknown relation `{}`", e.name, ed.kind) }); } }
            }
        }
        let label_kind = kinds.component(Label::NAME).expect("label");
        content_kinds.insert(label_kind);

        // 8. dead wires: readers/writers per component
        for c in &comps {
            let k = kinds.component(c.name).unwrap();
            let mut readers: Vec<String> = systems.iter().filter(|s| s.reads.contains(k)).map(|s| s.desc.name.to_string()).collect();
            readers.extend(rules.iter().filter(|r| r.signature().contains(k)).map(|r| format!("rule:{}", r.name())));
            let mut writers_n: Vec<String> = systems.iter().filter(|s| s.writes_all.contains(k) || s.writes_fields.iter().any(|(kk, _)| *kk == k)).map(|s| s.desc.name.to_string()).collect();
            writers_n.extend(systems.iter().filter(|s| s.spawns.contains(k)).map(|s| format!("spawn:{}", s.desc.name)));
            if content_kinds.contains(k) { writers_n.push("content".into()); }
            if k == label_kind { readers.push("engine".into()); writers_n.push("engine".into()); }
            if readers.is_empty() { defects.push(Defect::ComponentUnread { kind: c.name.into(), plugin: c.plugin.into() }); }
            if writers_n.is_empty() { defects.push(Defect::ComponentUnwritten { kind: c.name.into(), plugin: c.plugin.into() }); }
            report.components.push((c.name.into(), c.plugin.into(), readers, writers_n));
        }
        // creatable kinds, for rule bindability
        let mut creatable = content_kinds;
        for s in &systems { for k in s.writes_all.iter() { creatable.insert(k); } for k in s.spawns.iter() { creatable.insert(k); } }
        for r in &rules {
            for k in r.signature().iter() {
                if !creatable.contains(k) { defects.push(Defect::RuleUnbindable { rule: r.name().into(), kind: kinds.component_name(k).into() }); }
            }
        }

        // 9. events: handlers and schedulers
        for (name, ek) in kinds.events.iter() {
            let handlers: Vec<String> = systems.iter().filter(|s| s.handles.contains(ek)).map(|s| s.desc.name.to_string()).collect();
            let mut schedulers: Vec<String> = systems.iter().filter(|s| s.schedules.contains(ek)).map(|s| s.desc.name.to_string()).collect();
            if externals.contains(ek) { schedulers.push("external".into()); }
            if *name == EV_INIT || *name == EV_INVARIANT { schedulers.push("engine".into()); }
            // the invariant check and the law-switch are the engine's own: handled in `step`, never by a system
            let engine_handled = *name == EV_INVARIANT || *name == EV_LAW;
            if handlers.is_empty() && !engine_handled && *name != EV_INIT { defects.push(Defect::EventUnhandled { event: name.to_string() }); }
            if !handlers.is_empty() && schedulers.is_empty() { defects.push(Defect::EventUnscheduled { event: name.to_string(), handled_by: handlers.clone() }); }
            report.events.push((name.to_string(), handlers, schedulers));
        }
        report.relations = kinds.relations.iter().map(|(n, k)| (n.to_string(), kinds.rel_traits[k.0 as usize])).collect();

        // 10. THE RATCHET LAW: every bounded field has a raiser and a reachable lowerer
        for b in &bounded {
            let (kn, fname) = (b.decl.kind, b.decl.field);
            if b.decl.raised_by.is_empty() { defects.push(Defect::BoundedNoRaiser { kind: kn.into(), field: fname.into() }); }
            if b.decl.lowered_by.is_empty() { defects.push(Defect::BoundedNoLowerer { kind: kn.into(), field: fname.into() }); }
            for (m, is_lower) in b.decl.raised_by.iter().map(|m| (m, false)).chain(b.decl.lowered_by.iter().map(|m| (m, true))) {
                let Some(s) = systems.iter().find(|s| s.desc.name == m.system) else {
                    defects.push(Defect::MechanismInvalid { kind: kn.into(), field: fname.into(), system: m.system.into(), on: m.on.into(), reason: "no such system".into() });
                    continue;
                };
                if !s.may_write(b.kind, b.field) {
                    defects.push(Defect::MechanismInvalid { kind: kn.into(), field: fname.into(), system: m.system.into(), on: m.on.into(), reason: format!("system does not declare writing {kn}.{fname}") });
                }
                let Some(ek) = kinds.event(m.on) else {
                    defects.push(Defect::MechanismInvalid { kind: kn.into(), field: fname.into(), system: m.system.into(), on: m.on.into(), reason: "no such event".into() });
                    continue;
                };
                if !s.handles.contains(&ek) {
                    defects.push(Defect::MechanismInvalid { kind: kn.into(), field: fname.into(), system: m.system.into(), on: m.on.into(), reason: format!("system does not handle `{}`", m.on) });
                }
                let scheduled = systems.iter().any(|x| x.schedules.contains(&ek)) || externals.contains(&ek) || m.on == EV_INIT;
                if !scheduled && is_lower {
                    defects.push(Defect::LowererUnreachable { kind: kn.into(), field: fname.into(), system: m.system.into(), on: m.on.into() });
                }
            }
            report.bounded.push(format!("{kn}.{fname} ∈ [{}, {}] raised by {:?} lowered by {:?} τ={:?}", b.decl.min, b.decl.max,
                b.decl.raised_by.iter().map(|m| format!("{}@{}", m.system, m.on)).collect::<Vec<_>>(),
                b.decl.lowered_by.iter().map(|m| format!("{}@{}", m.system, m.on)).collect::<Vec<_>>(), b.decl.tau_hint));
        }

        // 11. loops: an internal brake must be a real (system, event)
        for l in &self.loops {
            if let Brake::Internal(m) = &l.brake {
                match systems.iter().find(|s| s.desc.name == m.system) {
                    None => defects.push(Defect::MechanismInvalid { kind: "loop".into(), field: l.name.into(), system: m.system.into(), on: m.on.into(), reason: "no such system".into() }),
                    Some(s) => match kinds.event(m.on) {
                        None => defects.push(Defect::MechanismInvalid { kind: "loop".into(), field: l.name.into(), system: m.system.into(), on: m.on.into(), reason: "no such event".into() }),
                        Some(ek) if !s.handles.contains(&ek) => defects.push(Defect::MechanismInvalid { kind: "loop".into(), field: l.name.into(), system: m.system.into(), on: m.on.into(), reason: format!("system does not handle `{}`", m.on) }),
                        _ => {}
                    },
                }
            }
            report.loops.push(format!("loop {} - {} ({}) brake: {:?} [{}]", l.number, l.name, if l.vicious { "vicious" } else { "virtuous" }, l.brake, l.source));
        }

        report.tunables = self.constants.tunables().iter().map(|s| s.to_string()).collect();
        report.contested = self.constants.contested().iter().map(|s| s.to_string()).collect();
        report.defects = defects;

        if !report.defects.is_empty() { return (None, report); }

        let mut h = blake3::Hasher::new();
        self.constants.hash_into(&mut h);
        for (_, t) in &self.proto_texts { h.update(t.as_bytes()); }
        for (_, t) in &self.scene_texts { h.update(t.as_bytes()); }
        for r in &rules { h.update(r.name().as_bytes()); h.update(&r.version().to_le_bytes()); }
        let content_hash = *h.finalize().as_bytes();
        let mut rv = blake3::Hasher::new();
        for r in &rules { rv.update(r.name().as_bytes()); rv.update(&r.version().to_le_bytes()); }
        let rule_versions = u32::from_le_bytes(rv.finalize().as_bytes()[..4].try_into().unwrap());

        let bp = Blueprint {
            kinds, components: comps, systems, rules, bounded, invariants, loops: self.loops.clone(), constants: self.constants.clone(), protos,
            scenes, plugins: self.plugins.clone(), externals, content_hash, rule_versions, report: report.clone(),
        };
        (Some(bp), report)
    }
}

/// Everything resolved; builds engines. Reusable (replay builds a second engine from the same blueprint).
pub struct Blueprint {
    pub kinds: Kinds,
    pub components: Vec<ComponentDecl>,
    pub systems: Vec<ResolvedSystem>,
    pub rules: Vec<Arc<dyn Rule>>,
    pub bounded: Vec<ResolvedBounded>,
    pub invariants: Vec<ResolvedInvariant>,
    pub loops: Vec<LoopDecl>,
    pub constants: Constants,
    pub protos: IndexMap<String, LoadedProto>,
    pub scenes: Vec<LoadedScene>,
    pub plugins: Vec<&'static str>,
    pub externals: Vec<EventKind>,
    pub content_hash: [u8; 32],
    pub rule_versions: u32,
    pub report: StaticReport,
}

impl Blueprint {
    pub fn build(&self, seed: u64, log: LogWriter, checkpoint_every: u64) -> Result<Engine, String> {
        Engine::from_blueprint(self, seed, log, checkpoint_every)
    }
}
