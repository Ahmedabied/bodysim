//! THE LAW-SWITCH: a constant amended, or a switch flipped, through the only door, and the replay law kept.
use bodysim_core::engine::Engine;
use bodysim_core::fixture;
use bodysim_core::log::LogWriter;
use bodysim_core::prelude::*;
use bodysim_core::time::{SimTime, Span};

fn engine(seed: u64) -> Engine {
    let bp = fixture::registry(false).finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    bp.build(seed, LogWriter::memory(), 100).unwrap()
}

#[test]
fn a_law_amends_a_constant_from_that_event_on_and_replays() {
    let mut e = engine(11);
    e.run_until(SimTime::ZERO + Span::hours(6));
    let was = e.world.k("fixture.tau_s");
    assert!(e.world.laws.is_empty() && !e.world.law("meal_place"));
    // through the door: logged, then landed at its event
    e.inject(EV_LAW, EntityId::WORLD, Payload::of(&LawSet { name: "fixture.tau_s".into(), value: was * 2.0 }), None).unwrap();
    e.inject(EV_LAW, EntityId::WORLD, Payload::of(&LawSet { name: "meal_place".into(), value: 1.0 }), None).unwrap();
    e.run_until(SimTime::ZERO + Span::hours(12));
    assert_eq!(e.world.k("fixture.tau_s"), was * 2.0, "the law is the constant now");
    assert_eq!(e.world.k_opt("fixture.tau_s"), Some(was * 2.0));
    assert!(e.world.law("meal_place"), "the switch is on");
    assert!(!e.world.law("not_a_law"));
    // the note names what changed and what it was
    assert!(e.log.records().any(|r| matches!(r, Record::Note { text, .. } if text.starts_with("law fixture.tau_s = ") && text.contains("(before: "))));
    e.run_until(SimTime::ZERO + Span::hours(30));
    e.checkpoint();
    // THE REPLAY LAW: the log replays, laws and all, to the same hash
    let records = e.log.memory.clone();
    let bp = fixture::registry(false).finalize().unwrap();
    let rep = Engine::replay(&bp, &records, LogWriter::discard()).unwrap();
    assert!(rep.ok(), "mismatches: {:?}", rep.mismatches);
    assert_eq!(rep.final_hash, e.hash());
    assert_eq!(rep.engine.as_ref().unwrap().world.k("fixture.tau_s"), was * 2.0, "the law survives the replay");
}

#[test]
fn a_world_without_laws_hashes_as_it_always_did() {
    // the laws map is hashed only once non-empty, so every checkpoint written before laws existed still matches
    let (mut a, mut b) = (engine(5), engine(5));
    a.run_until(SimTime::ZERO + Span::hours(20));
    b.run_until(SimTime::ZERO + Span::hours(20));
    assert_eq!(a.hash(), b.hash());
    b.inject(EV_LAW, EntityId::WORLD, Payload::of(&LawSet { name: "x".into(), value: 1.0 }), None).unwrap();
    b.run_until(SimTime::ZERO + Span::hours(20));
    assert_ne!(a.hash(), b.hash(), "a law in force is part of the state");
}
