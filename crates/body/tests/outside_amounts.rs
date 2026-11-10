//! A caffeine dose, a stressor and a meal come from outside the simulation, so their amounts are
//! checked where they come in.
//!
//! The engine refuses a non-finite number in a state write, but doses, cortisol pulses and meals
//! are stored inside lists, which that check cannot see into. A NaN dose would make
//! plasma caffeine NaN, then the sleep onset threshold NaN, and no onset would ever be found again.
use bodysim::cortisol::{baseline, cortisol_nmol, Cortisol, EV_STRESSOR};
use bodysim::digestion::{Digestion, Eat, MealKind, EV_EAT};
use bodysim::glucose::Glucose;
use bodysim::pk::{caffeine_mg_l, Caffeine, EV_DOSE};
use bodysim::sleep::{Sleep, SleepState};
use bodysim::{register_body, register_people};
use bodysim_core::prelude::*;

/// Inject one dose of `mg` at 08:00 on day one, then sample adult-a every 30 min for 48 h.
/// Returns how many doses she holds, her plasma caffeine at the end, whether she slept, and how
/// many refusals the log holds.
fn after_a_dose(mg: f64) -> (usize, f64, bool, usize) {
    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(1, LogWriter::memory(), 1000).expect("build");
    let id = e.world.find("adult-a").unwrap();
    let t0 = SimTime::ZERO + Span::hours(8);
    e.inject(EV_DOSE, id, Payload::of(&mg), Some(t0)).unwrap();
    let mut slept = false;
    let mut t = t0;
    for _ in 0..96 {
        t += Span::mins(30);
        e.run_until(t);
        slept |= e.world.get::<Sleep>(id).unwrap().state == SleepState::Asleep;
    }
    let refused = e.log.records().filter(|r| matches!(r, Record::Note { text, .. } if text.starts_with("dose refused"))).count();
    let c = e.world.get::<Caffeine>(id).unwrap();
    (c.doses.len(), caffeine_mg_l(c, t), slept, refused)
}

#[test]
fn a_dose_that_is_not_a_finite_amount_is_not_taken() {
    for mg in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY, -50.0] {
        let (doses, conc, slept, refused) = after_a_dose(mg);
        assert_eq!(doses, 0, "{mg} mg was stored as a dose");
        assert_eq!(refused, 1, "{mg} mg was dropped without a word in the log");
        assert!(conc.is_finite(), "{mg} mg left plasma caffeine at {conc}");
        assert!(slept, "{mg} mg: she never slept in the 48 h after it");
    }
    // nothing at all is a quiet no-op, not an error
    assert_eq!(after_a_dose(0.0), (0, -0.0, true, 0));
}

#[test]
fn a_real_dose_is_still_taken() {
    let (doses, conc, slept, refused) = after_a_dose(100.0);
    assert_eq!((doses, refused), (1, 0));
    assert!(conc > 0.0 && conc.is_finite(), "plasma caffeine {conc}");
    assert!(slept);
}

/// Inject a stressor of `intensity` at 10:00 on day one; adult-a's cortisol at 10:15, and how many
/// refusals the log holds.
fn after_a_stressor(intensity: f64) -> (f64, f64, usize) {
    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(1, LogWriter::memory(), 1000).expect("build");
    let id = e.world.find("adult-a").unwrap();
    let t0 = SimTime::ZERO + Span::hours(10);
    e.inject(EV_STRESSOR, id, Payload::of(&intensity), Some(t0)).unwrap();
    let t = t0 + Span::mins(15);
    e.run_until(t);
    let refused = e.log.records().filter(|r| matches!(r, Record::Note { text, .. } if text.starts_with("stressor refused"))).count();
    (cortisol_nmol(&e.world, e.world.get::<Cortisol>(id).unwrap(), t), baseline(&e.world, t), refused)
}

#[test]
fn a_stressor_of_no_number_is_refused() {
    // clamp passes NaN straight through, so it has to be named
    let (c, _, refused) = after_a_stressor(f64::NAN);
    assert!(c.is_finite(), "cortisol {c}");
    assert_eq!(refused, 1);
    // a real one still lands, and out of range is still clamped, not refused
    for x in [1.0, f64::INFINITY] {
        let (c, base, refused) = after_a_stressor(x);
        assert!(c > base + 1.0 && c.is_finite(), "{x}: cortisol {c}, baseline {base}");
        assert_eq!(refused, 0);
    }
}

/// Inject a meal at 08:00, then the 75 g glucose drink of the OGTT test at 12:00. Returns adult-a's
/// glucose rise 30 min after the drink, whether any stored meal holds a non-finite number, and how
/// many refusals the log holds.
fn after_a_meal(bad: Eat) -> (f64, bool, usize) {
    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(1, LogWriter::memory(), 1000).expect("build");
    let id = e.world.find("adult-a").unwrap();
    let t0 = SimTime::ZERO + Span::hours(8);
    e.inject(EV_EAT, id, Payload::of(&bad), Some(t0)).unwrap();
    let t1 = t0 + Span::hours(4);
    let drink = Eat { mass_g: 375.0, carb_g: 75.0, kj: 75.0 * 15.6, kind: MealKind::Liquid };
    e.inject(EV_EAT, id, Payload::of(&drink), Some(t1)).unwrap();
    e.run_until(t1);
    let before = e.world.get::<Glucose>(id).unwrap().g.at(t1);
    let t2 = t1 + Span::mins(30);
    e.run_until(t2);
    let rise = e.world.get::<Glucose>(id).unwrap().g.at(t2) - before;
    let poisoned = e.world.get::<Digestion>(id).unwrap().meals.iter().any(|m| !(m.mass_g.is_finite() && m.carb_g.is_finite() && m.kj.is_finite()));
    let refused = e.log.records().filter(|r| matches!(r, Record::Note { text, .. } if text.starts_with("meal refused"))).count();
    (rise, poisoned, refused)
}

#[test]
fn a_meal_with_an_amount_that_is_not_a_finite_amount_is_refused() {
    let ok = Eat { mass_g: 400.0, carb_g: 60.0, kj: 2500.0, kind: MealKind::Solid };
    for bad in [
        Eat { carb_g: f64::NAN, ..ok },
        Eat { mass_g: f64::INFINITY, ..ok },
        Eat { kj: f64::NAN, ..ok },
        Eat { carb_g: -60.0, ..ok },
    ] {
        let (rise, poisoned, refused) = after_a_meal(bad);
        assert!(!poisoned, "{bad:?} was stored as a meal");
        assert_eq!(refused, 1, "{bad:?} was dropped without a word in the log");
        assert!(rise > 20.0, "{bad:?}: the next drink raised glucose only {rise:.1} mg/dL");
    }
    let (rise, poisoned, refused) = after_a_meal(ok);
    assert!(!poisoned && refused == 0 && rise.is_finite(), "a real meal: rise {rise:.1}, refused {refused}");
}
