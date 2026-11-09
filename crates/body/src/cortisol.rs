//! Cortisol: a cosinor baseline plus event pulses - the awakening
//! response fires on the ACT of waking, not clock time, acute stress peaks ~12 min after the
//! stressor. Each pulse is a Bateman curve; the value is DERIVED as their sum; old pulses are pruned.
use crate::circadian::tod_hours;
use bodysim_core::component;
use bodysim_core::prelude::*;

component! {
    pub struct Cortisol = "cortisol" { pub pulses: Vec<Bateman> }
}

pub const EV_STRESSOR: &str = "stressor";

pub struct CortisolPlugin;

impl Plugin for CortisolPlugin {
    fn name(&self) -> &'static str { "cortisol" }
    fn register(&self, r: &mut Registry) {
        r.component::<Cortisol>();
        r.external_event(EV_STRESSOR);
        r.constants_ron("body.cortisol", include_str!("../../../content/constants/body/cortisol.ron"));
        r.system(SystemDescriptor {
            name: "hpa",
            reads: &["cortisol"],
            reads_opt: &["clock"],
            writes: &["cortisol"],
            handles: &["wake", EV_STRESSOR],
            consts: &["body.cortisol.car_nmol", "body.cortisol.car_rise_min", "body.cortisol.half_life_min",
                      "body.cortisol.stress_peak_nmol", "body.cortisol.stress_rise_min"],
            handler: hpa,
            ..Default::default()
        });
    }
}

/// Cosinor baseline, nmol/L serum.
pub fn baseline(w: &World, t: SimTime) -> f64 {
    let tod = tod_hours(w, t);
    w.k("body.cortisol.mesor_nmol") + w.k("body.cortisol.amplitude_nmol") * (std::f64::consts::TAU * (tod - w.k("body.cortisol.acrophase_h")) / 24.0).cos()
}
/// Total cortisol now: baseline + every pulse still alive.
pub fn cortisol_nmol(w: &World, c: &Cortisol, t: SimTime) -> f64 { baseline(w, t) + c.pulses.iter().map(|p| p.at(t)).sum::<f64>() }

/// A pulse rising to `peak` in `rise_min` and clearing with the plasma half-life: a Bateman with
/// ka chosen for the rise, ke from the half-life, dose/vd scaled to hit the peak.
fn pulse(peak: f64, rise_min: f64, half_life_min: f64, t0: SimTime) -> Bateman {
    let ke = (2f64).ln() / (half_life_min * 60.0);
    // t_peak = ln(ka/ke)/(ka−ke): solve ka by bisection on [ke·1.01, 1]
    let target = rise_min * 60.0;
    let f = |ka: f64| (ka / ke).ln() / (ka - ke) - target;
    let ka = bodysim_core::relax::bisect(f, ke * 1.0001, 1.0, 80);
    let mut b = Bateman { dose: 1.0, f: 1.0, ka, ke, vd: 1.0, t0 };
    let cmax = b.c_max().max(1e-12);
    b.dose = peak / cmax;
    b
}

fn hpa(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    let e = ev.subject;
    let Some(c) = w.get::<Cortisol>(e) else { return out };
    let mut c = c.clone();
    let hl = w.k("body.cortisol.half_life_min");
    match ev.kind_name {
        "wake" => c.pulses.push(pulse(w.k("body.cortisol.car_nmol"), w.k("body.cortisol.car_rise_min"), hl, now)),
        "stressor" => {
            let intensity: f64 = ev.decode().unwrap_or(1.0);
            c.pulses.push(pulse(w.k("body.cortisol.stress_peak_nmol") * intensity.clamp(0.0, 3.0), w.k("body.cortisol.stress_rise_min"), hl, now));
        }
        _ => return out,
    }
    // prune pulses that have decayed to nothing (8 half-lives)
    let horizon = Span::from_mins_f64(8.0 * hl);
    c.pulses.retain(|p| now - p.t0 < horizon);
    out.set(e, c);
    out
}
