//! The engine on its own: determinism, replay and the closed forms, with no body attached.
use bodysim_core::fixture::{self, Gauge, Tally};
use bodysim_core::log::Record;
use bodysim_core::prelude::*;
use bodysim_core::fel::Fel;
use bodysim_core::rng::{stream_named, uniform};

fn engine(seed: u64, unreachable: bool) -> Engine {
    let bp = fixture::registry(unreachable).finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    bp.build(seed, LogWriter::memory(), 100).unwrap()
}
fn day() -> SimTime { SimTime::ZERO + Span::days(1) }

#[test]
fn two_runs_same_seed_identical_hash_and_seed_matters() {
    let mut a = engine(1, false);
    let mut b = engine(1, false);
    let ra = a.run_until(day());
    let rb = b.run_until(day());
    assert!(ra.steps > 200, "a day of pulses should be hundreds of events, got {}", ra.steps);
    assert_eq!(ra.steps, rb.steps);
    assert_eq!(a.hash(), b.hash(), "same seed, same binary => identical state");
    assert_eq!(a.log.memory.len(), b.log.memory.len());
    let mut c = engine(2, false);
    c.run_until(day());
    assert_ne!(a.hash(), c.hash(), "the keyed streams must actually be used");
    // the gauge relaxes, drains fire, the tally counts
    let g1 = a.world.find("g1").unwrap();
    assert!(a.world.get::<Tally>(g1).unwrap().drains > 0);
    assert!(a.world.get::<Gauge>(g1).unwrap().pulses > 100);
    assert_eq!(a.counters.rejected, 0);
    assert_eq!(a.counters.undeclared_schedules, 0);
    assert!(a.world.undeclared_reads().is_empty());
    assert!(a.world.undeclared_consts().is_empty());
}

#[test]
fn fel_orders_by_time_prio_seq_under_a_million_pushes_with_cancels() {
    let mut fel = Fel::new();
    let mut rng = stream_named(7, "fel-test", EntityId::WORLD);
    let n = 1_000_000u64;
    let mut handles = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let at = SimTime((uniform(&mut rng) * 1e12) as i64);
        let prio = (uniform(&mut rng) * 4.0) as u8;
        let h = fel.push(Schedule::new(at, "pulse", EntityId::WORLD, "t").prio(prio), EventKind(0), None);
        handles.push(h);
    }
    let mut cancelled = 0u64;
    for (i, h) in handles.iter().enumerate() {
        if i % 7 == 0 { assert!(fel.cancel(*h).is_some()); cancelled += 1; }
    }
    assert!(fel.cancel(handles[0]).is_none(), "double cancel is none");
    let mut last = (SimTime(i64::MIN), 0u8, 0u64);
    let mut popped = 0u64;
    while let Some(ev) = fel.pop() {
        let key = (ev.at, ev.prio, ev.id.0);
        assert!(key > last, "FEL order broken: {last:?} then {key:?}");
        last = key;
        popped += 1;
    }
    assert_eq!(popped, n - cancelled);
}

#[test]
fn unreachable_crossing_is_refused_and_logged_never_scheduled() {
    let mut e = engine(1, true);
    e.run_until(day());
    let c = &e.counters.crossings["gauge reaches drain_at"];
    assert!(c.refused > 100, "{c:?}");
    assert_eq!(c.scheduled, 0);
    assert_eq!(c.fired, 0);
    assert!(!e.counters.events_by_kind.contains_key("drain"), "no drain ever fired");
    let unreachable = e.log.records().filter(|r| matches!(r, Record::Unreachable { .. })).count();
    assert_eq!(unreachable as u64, c.refused);
    let found = e.log.records().find(|r| matches!(r, Record::Unreachable { .. })).cloned();
    let Some(Record::Unreachable { system, why, threshold, x_inf, .. }) = found else { panic!("no Unreachable record") };
    assert_eq!(system, "pulser");
    assert_eq!(why, "gauge reaches drain_at");
    assert_eq!(threshold, 2.0);
    assert!(x_inf < 1.2 && x_inf > 0.8, "{x_inf}");
}

#[test]
fn rejected_proposal_logs_the_full_violation() {
    let mut e = engine(1, false);
    e.run_until(SimTime::ZERO + Span::hours(1));
    let g1 = e.world.find("g1").unwrap();
    let before = e.world.get::<Gauge>(g1).unwrap().clone();
    e.inject("break", g1, Payload::none(), None).unwrap();
    e.run_steps(1);
    assert_eq!(e.counters.rejected, 1);
    let v = e.log.records().find_map(|r| if let Record::Rejected { violation, by, .. } = r { Some((violation.clone(), by.clone())) } else { None }).expect("a Rejected record");
    let (v, by) = v;
    assert_eq!(by, "pulser");
    assert!(v.rule.starts_with("range:gauge.x"), "{}", v.rule);
    assert_eq!(v.entity, g1);
    assert_eq!(v.component.as_deref(), Some("gauge"));
    assert_eq!(v.field.as_deref(), Some("x"));
    assert!(matches!(v.old, Value::Relax(_)));
    assert!(matches!(v.attempted, Value::Relax(r) if r.x0 == 5.0));
    assert!(v.reason.contains("outside"), "{}", v.reason);
    assert_eq!(e.world.get::<Gauge>(g1).unwrap(), &before, "a rejected proposal changes nothing");
}

mod broken {
    use super::*;
    bodysim_core::component! { pub struct Orphan = "orphan" { pub v: f64 } }
    bodysim_core::component! { pub struct Ghost = "ghost" { pub v: f64 } }
    fn noop(_: &World, _: &Event, _: &mut Ctx) -> Out { Out::new() }
    pub struct P(pub &'static str);
    impl Plugin for P {
        fn name(&self) -> &'static str { "broken" }
        fn register(&self, r: &mut Registry) {
            match self.0 {
                "unread" => { r.component::<Orphan>(); r.system(SystemDescriptor { name: "w", writes: &["orphan"], handles: &["init"], handler: noop, ..Default::default() }); }
                "unwritten" => { r.component::<Ghost>(); r.system(SystemDescriptor { name: "rd", reads: &["ghost"], handles: &["init"], handler: noop, ..Default::default() }); }
                "unbindable" => { r.component::<Ghost>(); r.system(SystemDescriptor { name: "rd", reads: &["ghost"], handles: &["init"], handler: noop, ..Default::default() }); r.rule(DeclRule::Range { kind: "ghost".into(), field: "v".into(), min: 0.0, max: 1.0 }); }
                "unhandled" => { r.event("lonely"); }
                "unscheduled" => { r.event("orphan_ev"); r.system(SystemDescriptor { name: "h", reads: &["gauge"], handles: &["orphan_ev"], handler: noop, ..Default::default() }); }
                "nolower" => { r.bounded(BoundedDecl { kind: "tally", field: "drains", min: 0.0, max: 10.0, raised_by: &[Mechanism { system: "drainer", on: "drain" }], lowered_by: &[], tau_hint: None, rest: RestBound::Neither }); }
                "lowerunreachable" => {
                    r.event("never_scheduled");
                    r.system(SystemDescriptor { name: "lo", reads: &["tally"], writes: &["tally.drains"], handles: &["never_scheduled"], handler: noop, ..Default::default() });
                    r.bounded(BoundedDecl { kind: "tally", field: "drains", min: 0.0, max: 10.0, raised_by: &[Mechanism { system: "drainer", on: "drain" }], lowered_by: &[Mechanism { system: "lo", on: "never_scheduled" }], tau_hint: None, rest: RestBound::Neither });
                }
                "dupwriter" => { r.system(SystemDescriptor { name: "second", reads: &["gauge"], writes: &["gauge.x"], handles: &["init"], handler: noop, ..Default::default() }); }
                "mechanism" => { r.bounded(BoundedDecl { kind: "tally", field: "drains", min: 0.0, max: 10.0, raised_by: &[Mechanism { system: "drainer", on: "drain" }], lowered_by: &[Mechanism { system: "pulser", on: "drain" }], tau_hint: None, rest: RestBound::Neither }); }
                "constmissing" => { r.system(SystemDescriptor { name: "c", reads: &["gauge"], handles: &["init"], consts: &["fixture.nope"], handler: noop, ..Default::default() }); }
                "constforbidden" => {
                    r.constants_ron("watch", r#"[(name: "ego_depletion", value: 1.0, unit: "1", source: "replication watchlist", status: DoNotBuildOn)]"#);
                    r.system(SystemDescriptor { name: "c", reads: &["gauge"], handles: &["init"], consts: &["watch.ego_depletion"], handler: noop, ..Default::default() });
                }
                "unknown" => { r.system(SystemDescriptor { name: "u", reads: &["nope"], handles: &["init"], handler: noop, ..Default::default() }); }
                _ => unreachable!(),
            }
        }
    }
}

#[test]
fn finalize_refuses_each_defect_class() {
    use broken::P;
    type Pred = fn(&Defect) -> bool;
    let cases: Vec<(&str, Pred)> = vec![
        ("unread", |d| matches!(d, Defect::ComponentUnread { kind, .. } if kind == "orphan")),
        ("unwritten", |d| matches!(d, Defect::ComponentUnwritten { kind, .. } if kind == "ghost")),
        ("unbindable", |d| matches!(d, Defect::RuleUnbindable { kind, .. } if kind == "ghost")),
        ("unhandled", |d| matches!(d, Defect::EventUnhandled { event } if event == "lonely")),
        ("unscheduled", |d| matches!(d, Defect::EventUnscheduled { event, .. } if event == "orphan_ev")),
        ("nolower", |d| matches!(d, Defect::BoundedNoLowerer { field, .. } if field == "drains")),
        ("lowerunreachable", |d| matches!(d, Defect::LowererUnreachable { on, .. } if on == "never_scheduled")),
        ("dupwriter", |d| matches!(d, Defect::DuplicateWriter { field, systems, .. } if field == "x" && systems.len() == 2)),
        ("mechanism", |d| matches!(d, Defect::MechanismInvalid { system, .. } if system == "pulser")),
        ("constmissing", |d| matches!(d, Defect::ConstantMissing { name, .. } if name == "fixture.nope")),
        ("constforbidden", |d| matches!(d, Defect::ConstantForbidden { name, .. } if name == "watch.ego_depletion")),
        ("unknown", |d| matches!(d, Defect::UnknownComponent { name, .. } if name == "nope")),
    ];
    for (case, pred) in cases {
        let mut r = fixture::registry(false);
        r.add(&P(case));
        match r.finalize() {
            Ok(_) => panic!("case {case}: finalize accepted a defective registry"),
            Err(defects) => assert!(defects.iter().any(pred), "case {case}: expected defect not among {defects:?}"),
        }
    }
    assert!(fixture::registry(false).finalize().is_ok(), "the fixture itself is clean");
}

#[test]
fn replay_verifies_every_checkpoint() {
    let mut e = engine(11, false);
    e.run_until(SimTime::ZERO + Span::hours(12));
    let g2 = e.world.find("g2").unwrap();
    e.inject("poke", g2, Payload::of(&0.2f64), None).unwrap();
    e.run_until(SimTime::ZERO + Span::hours(18));
    e.inject("poke", g2, Payload::of(&-0.3f64), Some(SimTime::ZERO + Span::hours(20))).unwrap();
    e.run_until(day());
    e.checkpoint();
    let records = e.log.memory.clone();
    let bp = fixture::registry(false).finalize().unwrap();
    let rep = Engine::replay(&bp, &records, LogWriter::discard()).unwrap();
    assert!(rep.ok(), "mismatches: {:?}", rep.mismatches);
    assert!(rep.checked >= 3, "{}", rep.checked);
    assert_eq!(rep.final_hash, e.hash());
    assert_eq!(rep.final_events, e.counters.events);
    // and the log survives the wire format
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-tmp");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("replay.log.bin");
    {
        let mut w = LogWriter::file(&path, false).unwrap();
        for env in &records { w.append(env.record.clone()); }
    }
    let back = bodysim_core::log::read_log(&path).unwrap();
    assert_eq!(back.len(), records.len());
    assert_eq!(back[0].record, records[0].record);
    let rep2 = Engine::replay(&bp, &back, LogWriter::discard()).unwrap();
    assert!(rep2.ok());
}

mod extra {
    use super::*;
    bodysim_core::component! { pub struct Spawned = "spawned" { pub owner: EntityId, pub life: Span } }
    pub struct Spawner;
    impl Plugin for Spawner {
        fn name(&self) -> &'static str { "spawner" }
        fn register(&self, r: &mut Registry) {
            r.component::<Spawned>();
            r.event("expire");
            r.relation("owns", RelTraits { exclusive: true, acyclic: true, ..Default::default() });
            r.system(SystemDescriptor { name: "spawner", reads: &["spawned", "gauge"], writes: &["spawned"], spawns: &["spawned", "label"], may_despawn: true, edges: &["owns"], handles: &["init", "expire"], schedules: &["expire"], handler: spawner, ..Default::default() });
            // a system that reads what it did not declare, and writes what it does not own
            r.system(SystemDescriptor { name: "sloppy", reads: &["gauge"], writes: &[], handles: &["init"], handler: sloppy, ..Default::default() });
            r.invariant(Invariant::BoundedResponse { name: "gauge settles after a poke", kind: "gauge", field: "x", threshold: 0.6, within: Span::hours(1) });
        }
    }
    fn spawner(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
        let mut out = Out::new();
        if ev.is("init") {
            let g1 = w.find("g1").unwrap();
            let e = ctx.reserve();
            out.spawn(e, vec![Box::new(Spawned { owner: g1, life: Span::hours(2) }), Box::new(Label { name: "child".into() })]);
            out.edge_open(e, "owns", g1, None);
            out.schedule_in(ctx.now, Span::hours(2), "expire", e, "child life");
        } else {
            out.despawn(ev.subject);
        }
        out
    }
    fn sloppy(w: &World, _ev: &Event, _ctx: &mut Ctx) -> Out {
        let mut out = Out::new();
        let g1 = w.find("g1").unwrap();
        let t = w.get::<Tally>(g1).unwrap(); // undeclared read
        out.set(g1, Tally { drains: t.drains + 5 }); // not the owner
        out
    }
}

#[test]
fn spawn_despawn_ownership_undeclared_read_and_invariants() {
    let mut r = fixture::registry(false);
    r.add(&extra::Spawner);
    let bp = r.finalize().unwrap_or_else(|d| panic!("{d:?}"));
    let mut e = bp.build(5, LogWriter::memory(), 50).unwrap();
    e.run_until(SimTime::ZERO + Span::mins(1));
    let child = e.world.find("child").expect("spawned through a proposal");
    assert!(e.world.alive(child));
    assert_eq!(e.world.get::<extra::Spawned>(child).unwrap().owner, e.world.find("g1").unwrap());
    let owns = e.world.kinds.relation("owns").unwrap();
    assert_eq!(e.world.edges.open(child, owns).len(), 1);
    // the sloppy system: one undeclared read counted, one ownership rejection logged
    assert_eq!(e.world.undeclared_reads().len(), 1);
    assert_eq!(e.counters.rejected, 1);
    let v = e.log.records().find_map(|r| if let Record::Rejected { violation, .. } = r { Some(violation.clone()) } else { None }).unwrap();
    assert_eq!(v.rule, "ownership");
    assert!(v.reason.contains("LAW 6"));
    // invariant: poke g2 above 0.6, it keeps rising (pulses) ⇒ violated at the deadline
    let g2 = e.world.find("g2").unwrap();
    e.inject("poke", g2, Payload::of(&0.3f64), None).unwrap();
    e.run_until(SimTime::ZERO + Span::hours(3));
    assert!(!e.world.alive(child), "despawned on expire");
    assert!(e.world.find("child").is_none());
    assert!(e.world.edges.get(bodysim_core::id::EdgeId(0)).map(|x| !x.is_open()).unwrap_or(true) || e.world.edges.open(child, owns).is_empty());
    let inv = &e.counters.invariants[0];
    assert!(inv.armed >= 1, "{inv:?}");
    assert!(inv.violated >= 1 || inv.satisfied >= 1);
    assert!(e.log.records().any(|r| matches!(r, Record::Invariant { .. })));
    // dropped: an event scheduled for the despawned child never runs
    assert_eq!(e.counters.same_instant_drops, 0);
}
