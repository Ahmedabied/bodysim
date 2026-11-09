//! A 75 g oral glucose tolerance test, the calibration check for `mg_dl_per_g_carb`.
//!
//! Targets from Hirakawa et al. 2020 (Hisayama Study, BMJ Open Diabetes Res Care 8:e001156), Table 1:
//! the healthiest two quartiles by 30 min glucose (at or below 135, and 136 to 157 mg/dL) had a 2 h
//! value 15.2 and 21.1 mg/dL above their fasting value. Normal glucose tolerance (WHO/IDF 2006) means
//! fasting below 110 and 2 h below 140 mg/dL.
use bodysim::digestion::{Eat, MealKind, EV_EAT};
use bodysim::glucose::Glucose;
use bodysim::{register_body, register_people};
use bodysim_core::prelude::*;

fn ogtt(who: &str) -> (f64, f64, f64) {
    let mut r = Registry::new();
    register_body(&mut r);
    register_people(&mut r);
    let bp = r.finalize().unwrap_or_else(|d| panic!("defects: {d:?}"));
    let mut e = bp.build(1, LogWriter::memory(), 1000).expect("build");
    let id = e.world.find(who).unwrap();
    // 75 g of glucose in 300 mL of water, after a night without food.
    let t0 = SimTime::ZERO + Span::hours(8);
    let drink = Eat { mass_g: 375.0, carb_g: 75.0, kj: 75.0 * 15.6, kind: MealKind::Liquid };
    e.inject(EV_EAT, id, Payload::of(&drink), Some(t0)).unwrap();
    let mut at = |t: SimTime| {
        e.run_until(t);
        e.world.get::<Glucose>(id).unwrap().g.at(t)
    };
    (at(t0), at(t0 + Span::mins(30)), at(t0 + Span::mins(120)))
}

#[test]
fn a_75_g_glucose_drink_matches_the_published_curve() {
    for who in ["adult-a", "adult-b", "adult-c"] {
        let (fasting, m30, m120) = ogtt(who);
        println!("{who}: fasting {fasting:.1}, 30 min {m30:.1}, 2 h {m120:.1} mg/dL");
        assert!(fasting < 110.0 && m120 < 140.0, "{who}: not normal glucose tolerance");
        assert!((fasting + 30.0..=157.0).contains(&m30), "{who}: 30 min {m30:.1} outside the two healthiest quartiles");
        let rise = m120 - fasting;
        assert!((10.0..=25.0).contains(&rise), "{who}: 2 h rise {rise:.1}, published 15.2 to 21.1");
    }
}
