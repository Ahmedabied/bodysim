//! Hard problem #4: the ECS overhead is unmeasured. Target: 10⁴ entities,
//! 10⁵ events per simulated day, < 1 s per day at `--speed max`. Run with `--release`.
use bodysim_core::fixture::{FixturePlugin, Gauge, Tally};
use bodysim_core::prelude::*;

#[test]
#[ignore = "run with: cargo test --release -p bodysim-core --test scale -- --ignored --nocapture"]
#[allow(clippy::disallowed_methods)] // a benchmark measures wall time; the simulation never reads it
fn ten_thousand_gauges_one_day() {
    let n = 10_000;
    let mut scene = String::from("(entities: [");
    for i in 0..n {
        scene.push_str(&format!("(name: \"g{i}\", proto: \"gauge_thing\"),"));
    }
    scene.push_str("])");
    let mut r = Registry::new();
    r.add(&FixturePlugin::default());
    // the fixture's own 3-entity scene plus ours: constants pulse every 600 s → ~1.4e6 pulses/day
    // for 10⁴ gauges; make the period 4 h so the day is ~10⁵ events
    r.constants_ron("scale", "[]");
    r.scene_ron(&scene);
    let bp = r.finalize().unwrap_or_else(|d| panic!("{d:?}"));
    let mut e = bp.build(1, LogWriter::discard(), 100_000).unwrap();
    let t0 = std::time::Instant::now();
    let rep = e.run_until(SimTime::ZERO + Span::hours(2));
    let dt = t0.elapsed();
    let per_event = dt.as_secs_f64() / rep.steps as f64;
    eprintln!("{} entities · {} events in {:?} → {:.2} µs/event → a 10⁵-event day would take {:.3} s",
        e.world.entities.count_alive(), rep.steps, dt, per_event * 1e6, per_event * 1e5);
    assert!(e.world.get::<Gauge>(e.world.find("g9999").unwrap()).is_some());
    assert!(e.world.get::<Tally>(e.world.find("g0").unwrap()).is_some());
    assert!(per_event * 1e5 < 1.0, "a 10⁵-event day would take {:.3} s", per_event * 1e5);
}
