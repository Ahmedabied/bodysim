//! A week of the three example adults: deterministic, replayable, and inside physiological ranges.
use bodysim::cortisol::{cortisol_nmol, Cortisol};
use bodysim::digestion::{Eat, MealKind, EV_EAT};
use bodysim::glucose::Glucose;
use bodysim::hydration::EV_VOID;
use bodysim::thermo::Thermo;
use bodysim::{register_body, register_people};
use bodysim_core::prelude::*;
use bodysim_core::relax::Trajectory;

const PEOPLE: [&str; 3] = ["adult-a", "adult-b", "adult-c"];

fn week(seed: u64) -> (Engine, Vec<(String, f64, f64, f64)>) {
    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(seed, LogWriter::memory(), 1000).expect("build");
    for d in 0..7 {
        for name in PEOPLE {
            let id = e.world.find(name).unwrap();
            for (h, carb) in [(7.5, 60.0), (12.5, 75.0), (19.0, 80.0)] {
                let at = SimTime::ZERO + Span::days(d) + Span::secs((h * 3600.0) as i64);
                let eat = Eat { mass_g: 550.0, carb_g: carb, kj: 2800.0, kind: MealKind::Solid };
                e.inject(EV_EAT, id, Payload::of(&eat), Some(at)).unwrap();
            }
            for h in [7.0, 11.0, 15.0, 19.0, 23.0] {
                e.inject(EV_VOID, id, Payload::none(), Some(SimTime::ZERO + Span::days(d) + Span::secs(h as i64 * 3600))).unwrap();
            }
        }
    }
    let mut samples = Vec::new();
    let mut t = SimTime::ZERO;
    while t <= SimTime::ZERO + Span::days(7) {
        e.run_until(t);
        for name in PEOPLE {
            let id = e.world.find(name).unwrap();
            let c = cortisol_nmol(&e.world, e.world.get::<Cortisol>(id).unwrap(), t);
            let core = e.world.get::<Thermo>(id).unwrap().state.state_at(t)[1];
            let g = e.world.get::<Glucose>(id).unwrap().g.at(t);
            samples.push((name.to_string(), c, core, g));
        }
        t += Span::secs(900);
    }
    (e, samples)
}

#[test]
fn a_week_is_deterministic_and_physiological() {
    let (a, sa) = week(7);
    let (b, sb) = week(7);
    assert_eq!(a.hash(), b.hash(), "same seed, same state");
    assert_eq!(sa, sb, "same seed, same trajectories");
    // A refused write freezes a component while its scheduled events keep firing (a full colon once
    // froze digestion and re-absorbed the same meal over and over), so a week must refuse nothing.
    let c = &a.counters;
    assert_eq!((c.rejected, c.undeclared_schedules, c.unknown_schedules), (0, 0, 0), "refused writes or schedules: {c:?}");
    for (who, c, core, g) in &sa {
        assert!((36.0..=37.6).contains(core), "{who}: core {core:.2} C outside the normal daily range");
        assert!((60.0..=220.0).contains(g), "{who}: glucose {g:.0} mg/dL implausible");
        assert!((50.0..=900.0).contains(c), "{who}: cortisol {c:.0} nmol/L implausible");
    }
    // Every adult has a cortisol peak in the morning and a glucose rise after meals.
    for who in PEOPLE {
        let day2: Vec<_> = sa.iter().filter(|s| s.0 == who).skip(96).take(96).collect();
        let peak_hour = day2.iter().enumerate().max_by(|x, y| x.1 .1.total_cmp(&y.1 .1)).unwrap().0 as f64 / 4.0;
        assert!((5.0..=11.0).contains(&peak_hour), "{who}: cortisol peaks at {peak_hour} h");
        let fasting = day2[(6.5 * 4.0) as usize].3;
        let after_lunch = day2.iter().skip(13 * 4).take(8).map(|s| s.3).fold(0.0, f64::max);
        assert!(after_lunch > fasting + 15.0, "{who}: no glucose response to lunch");
        let day_max = day2.iter().map(|s| s.3).fold(0.0, f64::max);
        assert!(day_max < 140.0, "{who}: glucose {day_max:.0} mg/dL after an ordinary meal");
    }
}
