//! Print a simulated day (or more) of the three example adults as CSV, one row per person every 10 minutes.
//!
//!     cargo run --release --example day -- [days] [seed] > day.csv
//!
//! Each adult gets breakfast at 07:30, lunch at 12:30 and dinner at 19:00 (mixed solid meals), so glucose has
//! something to answer, goes to the toilet at 07, 11, 15, 19 and 23 h (and whenever the bladder or bowel
//! presses), and drinks 250 mL every two hours from 08 to 22 h, so a run of more than a week does not dry out
//! (the refused writes go to stderr, and stay 0). Everything else (sleep, waking, the cortisol awakening response, core temperature) comes from the
//! models.
use bodysim::cortisol::{cortisol_nmol, Cortisol};
use bodysim::digestion::{Eat, MealKind, EV_EAT};
use bodysim::glucose::Glucose;
use bodysim::hydration::EV_VOID;
use bodysim::DrinkPayload;
use bodysim::thermo::Thermo;
use bodysim::{register_body, register_people};
use bodysim_core::prelude::*;
use bodysim_core::relax::Trajectory;

pub const PEOPLE: [&str; 3] = ["adult-a", "adult-b", "adult-c"];

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let days: i64 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(1);
    let seed: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(1);

    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(seed, LogWriter::discard(), 1000).expect("build");

    let meals = [(7.5, 450.0, 60.0, 2300.0), (12.5, 600.0, 75.0, 3000.0), (19.0, 650.0, 80.0, 3300.0)];
    for d in 0..days {
        for name in PEOPLE {
            let id = e.world.find(name).expect("person");
            for (h, mass_g, carb_g, kj) in meals {
                let at = SimTime::ZERO + Span::days(d) + Span::secs((h * 3600.0) as i64);
                let eat = Eat { mass_g, carb_g, kj, kind: MealKind::Solid };
                e.inject(EV_EAT, id, Payload::of(&eat), Some(at)).expect("eat");
            }
            for h in [7.0, 11.0, 15.0, 19.0, 23.0] {
                let at = SimTime::ZERO + Span::days(d) + Span::secs((h * 3600.0) as i64);
                e.inject(EV_VOID, id, Payload::none(), Some(at)).expect("void");
            }
            for h in [8, 10, 12, 14, 16, 18, 20, 22] {
                let drink = DrinkPayload { by: id, volume_l: 0.25 };
                e.inject("drink", id, Payload::of(&drink), Some(SimTime::ZERO + Span::days(d) + Span::secs(h * 3600))).expect("drink");
            }
        }
    }

    println!("hour,person,cortisol_nmol_l,core_c,glucose_mg_dl");
    let end = SimTime::ZERO + Span::days(days);
    let mut t = SimTime::ZERO;
    while t <= end {
        e.run_until(t);
        for name in PEOPLE {
            let id = e.world.find(name).expect("person");
            let c = e.world.get::<Cortisol>(id).map(|c| cortisol_nmol(&e.world, c, t)).unwrap_or(f64::NAN);
            let core = e.world.get::<Thermo>(id).map(|th| th.state.state_at(t)[1]).unwrap_or(f64::NAN);
            let g = e.world.get::<Glucose>(id).map(|g| g.g.at(t)).unwrap_or(f64::NAN);
            println!("{:.3},{name},{c:.2},{core:.3},{g:.1}", (t - SimTime::ZERO).as_secs_f64() / 3600.0);
        }
        t += Span::secs(600);
    }
    eprintln!("refused writes: {}", e.counters.rejected);
}
