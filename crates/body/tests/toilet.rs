//! Nobody uses the toilet in their sleep. Asleep, a pressing urge waits; an urgent one wakes the sleeper
//! (nocturia), and what presses on waking is answered then. Measured before this: in the bench's ordinary day
//! (three meals, toilet visits, eight drinks) each adult voided or defecated in their sleep on most nights.
use bodysim::digestion::{Eat, MealKind, EV_EAT};
use bodysim::hydration::EV_VOID;
use bodysim::{register_body, register_people, DrinkPayload};
use bodysim_core::log::ChangeRecord;
use bodysim_core::prelude::*;
use bodysim_core::value::Value;

const DAYS: i64 = 10;

fn ordinary_days() -> Engine {
    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(1, LogWriter::memory(), 100_000).expect("build");
    for d in 0..DAYS {
        for name in ["adult-a", "adult-b", "adult-c"] {
            let id = e.world.find(name).unwrap();
            for (h, carb) in [(7.5, 60.0), (12.5, 75.0), (19.0, 80.0)] {
                let at = SimTime::ZERO + Span::days(d) + Span::secs((h * 3600.0) as i64);
                e.inject(EV_EAT, id, Payload::of(&Eat { mass_g: 550.0, carb_g: carb, kj: 2800.0, kind: MealKind::Solid }), Some(at)).unwrap();
            }
            for h in [10, 14, 18, 22] {
                e.inject(EV_VOID, id, Payload::none(), Some(SimTime::ZERO + Span::days(d) + Span::secs(h * 3600))).unwrap();
            }
            for h in [8, 10, 12, 14, 16, 18, 20, 22] {
                let drink = DrinkPayload { by: id, volume_l: 0.25 };
                e.inject("drink", id, Payload::of(&drink), Some(SimTime::ZERO + Span::days(d) + Span::secs(h * 3600))).unwrap();
            }
        }
    }
    e.run_until(SimTime::ZERO + Span::days(DAYS));
    e
}

#[test]
fn nobody_uses_the_toilet_asleep_and_nights_stay_whole() {
    let e = ordinary_days();
    assert_eq!(e.counters.rejected, 0, "refused writes");
    for name in ["adult-a", "adult-b", "adult-c"] {
        let id = e.world.find(name).unwrap();
        // follow the sleep state itself (a return to bed after a night visit has no `sleep_onset` event)
        let mut asleep_since: Option<SimTime> = None;
        let mut slept_by_night = vec![0.0f64; DAYS as usize + 1];
        let mut visits = 0;
        let mut now = SimTime::ZERO;
        for rec in e.log.records() {
            match rec {
                Record::Event { at, kind, subject, .. } => {
                    now = *at;
                    if *subject == id && (kind == "void" || kind == "defecate") {
                        visits += 1;
                        assert!(asleep_since.is_none(), "{name}: `{kind}` at {at} while asleep");
                    }
                }
                Record::Applied { entity, change: ChangeRecord::Set { kind, fields }, .. } if *entity == id && kind == "sleep" => {
                    for (field, _, new) in fields {
                        if field != "state" { continue; }
                        match (new, asleep_since) {
                            (Value::Enum(1), None) => asleep_since = Some(now),
                            (Value::Enum(0), Some(s)) => {
                                // a night runs noon to noon
                                let night = ((s - SimTime::ZERO).as_hours_f64() + 12.0).div_euclid(24.0) as usize;
                                slept_by_night[night] += (now - s).as_hours_f64();
                                asleep_since = None;
                            }
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        assert!(visits > DAYS * 4, "{name}: only {visits} toilet visits in {DAYS} days");
        // nights 1 to DAYS-1 are whole nights inside the run
        for (n, h) in slept_by_night.iter().enumerate().take(DAYS as usize).skip(1) {
            assert!(*h >= 6.0, "{name}: night {n} held only {h:.1} h of sleep");
        }
    }
}
