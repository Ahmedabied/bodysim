//! Process C and melatonin. The circadian
//! oscillator is a FORMULA of time and the person's phase; melatonin a cosinor envelope suppressed
//! by light through a logistic (ED50 ~106 lux, max 93.6 %). Nothing is stepped; nothing is polled.
use crate::person::Body;
use bodysim_core::component;
use bodysim_core::prelude::*;

component! {
    /// The person's clock: `peak_hour` is when Process C peaks - mid-afternoon, so that the
    /// sleep thresholds are lowest around the core-temperature minimum twelve hours later
    /// (03:00-05:00); `tau_h` the intrinsic period. Chronotype = this hour.
    pub struct Circadian = "circadian" { pub peak_hour: f64, pub tau_h: f64 }
}

pub struct CircadianPlugin;

impl Plugin for CircadianPlugin {
    fn name(&self) -> &'static str { "circadian" }
    fn register(&self, r: &mut Registry) {
        r.component::<Circadian>();
        r.constants_ron("body.circadian", include_str!("../../../content/constants/body/circadian.ron"));
        r.rule(DeclRule::Range { kind: "circadian".into(), field: "tau_h".into(), min: 23.5, max: 26.0 });
        r.rule(DeclRule::Range { kind: "circadian".into(), field: "peak_hour".into(), min: 0.0, max: 24.0 });
        r.rule(DeclRule::Requires { kind: "circadian".into(), other: "body".into() });
    }
}

/// Local time of day in hours, from the world clock if present, else from the epoch.
pub fn tod_hours(w: &World, t: SimTime) -> f64 {
    let epoch = match (w.field_named(EntityId::WORLD, "clock", "epoch_unix_secs"), w.field_named(EntityId::WORLD, "clock", "tz_offset_secs")) {
        (Some(Value::I64(u)), Some(Value::I64(tz))) => CivilEpoch { unix_secs: u, tz_offset_secs: tz as i32 },
        _ => CivilEpoch { unix_secs: 0, tz_offset_secs: 0 },
    };
    t.civil(epoch).tod_hours()
}

/// Process C ∈ [−1, 1] for this person at `t`.
pub fn process_c(w: &World, e: EntityId, t: SimTime) -> f64 {
    let Some(c) = w.get::<Circadian>(e) else { return 0.0 };
    let tod = tod_hours(w, t);
    (std::f64::consts::TAU * (tod - c.peak_hour) / c.tau_h).cos()
}

/// Melatonin (pg/mL): cosinor envelope × light suppression (logistic on melanopic lux at the eye).
pub fn melatonin_pg_ml(w: &World, e: EntityId, t: SimTime, medi_lux: f64) -> f64 {
    let Some(c) = w.get::<Circadian>(e) else { return 0.0 };
    let mesor = w.k("body.circadian.mel_mesor_pg_ml");
    let amp = w.k("body.circadian.mel_amplitude_pg_ml");
    // acrophase sits ~8.5 h after the C peak (02:27 for a 18:00 peak)
    let acro = c.peak_hour + w.k("body.circadian.mel_acrophase_after_peak_h");
    let tod = tod_hours(w, t);
    let env = (mesor + amp * (std::f64::consts::TAU * (tod - acro) / c.tau_h).cos()).max(0.0);
    let ed50 = w.k("body.circadian.mel_suppression_ed50_lux");
    let d = w.k("body.circadian.mel_suppression_steepness");
    let max = w.k("body.circadian.mel_suppression_max");
    let supp = max / (1.0 + (ed50 / medi_lux.max(1e-6)).powf(d));
    env * (1.0 - supp)
}

pub fn has_body(w: &World, e: EntityId) -> bool { w.get::<Body>(e).is_some() }
