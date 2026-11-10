//! The bench's ordinary day (three meals, five toilet visits, eight 250 mL drinks) keeps a body healthy: no write
//! is refused. Without the drinks the water deficit reaches its bound in about a week and writes are refused,
//! which is what the speed bench measured until 2026-11-10.
use bodysim::digestion::{Eat, MealKind, EV_EAT};
use bodysim::hydration::EV_VOID;
use bodysim::{register_body, register_people, DrinkPayload};
use bodysim_core::prelude::*;

fn refused_after(days: i64, drinks: bool) -> u64 {
    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(1, LogWriter::discard(), 100_000).expect("build");
    let at = |d: i64, h: f64| SimTime::ZERO + Span::days(d) + Span::secs((h * 3600.0) as i64);
    for d in 0..days {
        for name in ["adult-a", "adult-b", "adult-c"] {
            let id = e.world.find(name).unwrap();
            for (h, carb) in [(7.5, 60.0), (12.5, 75.0), (19.0, 80.0)] {
                e.inject(EV_EAT, id, Payload::of(&Eat { mass_g: 550.0, carb_g: carb, kj: 2800.0, kind: MealKind::Solid }), Some(at(d, h))).unwrap();
            }
            for h in [7.0, 11.0, 15.0, 19.0, 23.0] { e.inject(EV_VOID, id, Payload::none(), Some(at(d, h))).unwrap(); }
            if drinks {
                for h in [8.0, 10.0, 12.0, 14.0, 16.0, 18.0, 20.0, 22.0] {
                    e.inject("drink", id, Payload::of(&DrinkPayload { by: id, volume_l: 0.25 }), Some(at(d, h))).unwrap();
                }
            }
        }
    }
    e.run_until(SimTime::ZERO + Span::days(days));
    e.counters.rejected
}

#[test]
fn the_ordinary_day_refuses_no_write_and_without_water_it_does() {
    assert_eq!(refused_after(10, true), 0, "the bench's ordinary day refused writes");
    assert!(refused_after(10, false) > 0, "ten days without water refused nothing: the check above proves nothing");
}
