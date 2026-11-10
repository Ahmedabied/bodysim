//! How fast the simulation runs: simulated days per second of wall time, three adults with meals, drinks and
//! toilet visits, no sampling (the engine only wakes on events and threshold crossings). The run must stay
//! healthy: it prints the writes the rules refused, and that number is 0.
//!
//!     cargo run --release --example bench -- [days]
//!
//! The wall clock is read here, outside the simulation, which is the only place it is allowed.
#![allow(clippy::disallowed_methods)]
use bodysim::digestion::{Eat, MealKind, EV_EAT};
use bodysim::hydration::EV_VOID;
use bodysim::{register_body, register_people, DrinkPayload};
use bodysim_core::prelude::*;
use std::time::Instant;

fn main() {
    let days: i64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(365);
    let t0 = Instant::now();
    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(1, LogWriter::discard(), 100_000).expect("build");
    for d in 0..days {
        for name in ["adult-a", "adult-b", "adult-c"] {
            let id = e.world.find(name).expect("person");
            for (h, carb) in [(7.5, 60.0), (12.5, 75.0), (19.0, 80.0)] {
                let at = SimTime::ZERO + Span::days(d) + Span::secs((h * 3600.0) as i64);
                let eat = Eat { mass_g: 550.0, carb_g: carb, kj: 2800.0, kind: MealKind::Solid };
                e.inject(EV_EAT, id, Payload::of(&eat), Some(at)).expect("eat");
            }
            for h in [7, 11, 15, 19, 23] {
                e.inject(EV_VOID, id, Payload::none(), Some(SimTime::ZERO + Span::days(d) + Span::secs(h * 3600))).expect("void");
            }
            // about 2 L a day: without water the deficit reaches its bound in a week and every water write is refused
            for h in [8, 10, 12, 14, 16, 18, 20, 22] {
                let drink = DrinkPayload { by: id, volume_l: 0.25 };
                e.inject("drink", id, Payload::of(&drink), Some(SimTime::ZERO + Span::days(d) + Span::secs(h * 3600))).expect("drink");
            }
        }
    }
    let setup = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let rep = e.run_until(SimTime::ZERO + Span::days(days));
    let run = t1.elapsed().as_secs_f64();
    println!("{days} simulated days, 3 adults: {} events in {:.3} s (setup {:.3} s)", rep.steps, run, setup);
    println!("{:.1} simulated days per second, {:.0} events per second", days as f64 / run, rep.steps as f64 / run);
    let hash: String = rep.hash.iter().take(8).map(|b| format!("{b:02x}")).collect();
    println!("state hash {hash} (the same for every run with the same days)");
    println!("refused writes: {}", e.counters.rejected);
}
