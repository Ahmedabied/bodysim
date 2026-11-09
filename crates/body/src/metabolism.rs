//! Energy: BMR (Mifflin-St Jeor) × MET by activity; the glycogen buffer as a `Linear`
//! (consumed at the carbohydrate share of expenditure, refilled by absorbed carbs); fat mass as the
//! Hall/Chow closed form (half-life ≈ 1 year, re-anchored daily on the day's balance).
//!
use crate::person::{Body, Sex};
use bodysim_core::component;
use bodysim_core::prelude::*;
use serde::{Deserialize, Serialize};

component! {
    /// `glycogen_g` a linear buffer; `fat_kg` relaxes toward the balance's equilibrium;
    /// `intake_kj_today` the running meal total (reset at midnight).
    pub struct Metabolism = "metabolism" { pub glycogen_g: Linear, pub fat_kg: Relax, pub lean_kg: f64, pub intake_kj_today: f64 }
}

/// Emitted by digestion as a meal empties: what reached the blood in this window.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Absorbed { pub carb_g: f64, pub kj: f64 }

pub const EV_ABSORBED: &str = "absorbed";
pub const EV_ACTIVITY: &str = "activity_set";

pub struct MetabolismPlugin;

impl Plugin for MetabolismPlugin {
    fn name(&self) -> &'static str { "metabolism" }
    fn register(&self, r: &mut Registry) {
        r.component::<Metabolism>();
        r.event(EV_ABSORBED);
        r.constants_ron("body.metabolism", include_str!("../../../content/constants/body/metabolism.ron"));
        r.system(SystemDescriptor {
            name: "energy",
            reads: &["metabolism", "body"],
            writes: &["metabolism"],
            handles: &["init", EV_ABSORBED, EV_ACTIVITY, "sleep_onset", "wake"],
            handles_opt: &["midnight"],
            consts: &["body.metabolism.msj_w", "body.metabolism.msj_h", "body.metabolism.msj_a", "body.metabolism.msj_f", "body.metabolism.msj_m",
                      "body.metabolism.kcal_to_kj", "body.metabolism.carb_share_rest", "body.metabolism.glycogen_kj_per_g",
                      "body.metabolism.weight_tau_days", "body.metabolism.kj_per_day_per_kg", "body.metabolism.glycogen_cap_g"],
            handler: energy,
            ..Default::default()
        });
        r.bounded(BoundedDecl {
            kind: "metabolism", field: "glycogen_g", min: 0.0, max: 700.0,
            raised_by: &[Mechanism { system: "energy", on: EV_ABSORBED }],
            lowered_by: &[Mechanism { system: "energy", on: EV_ACTIVITY }],
            tau_hint: Some(Span::hours(24)), rest: RestBound::Neither,
        });
        r.rule(DeclRule::Range { kind: "metabolism".into(), field: "fat_kg".into(), min: 1.0, max: 150.0 });
    }
}

/// Basal metabolic rate, W (Mifflin-St Jeor, kcal/day → W).
pub fn bmr_w(w: &World, b: &Body) -> f64 {
    let sex = if b.sex == Sex::Male { w.k("body.metabolism.msj_m") } else { w.k("body.metabolism.msj_f") };
    let kcal = w.k("body.metabolism.msj_w") * b.mass_kg + w.k("body.metabolism.msj_h") * b.height_cm - w.k("body.metabolism.msj_a") * b.age_yr + sex;
    kcal * w.k("body.metabolism.kcal_to_kj") * 1000.0 / 86_400.0
}
/// Total expenditure right now, W.
pub fn expenditure_w(w: &World, b: &Body) -> f64 { bmr_w(w, b) * b.met() }

fn reanchor_glycogen(w: &World, b: &Body, m: &Metabolism, now: SimTime) -> Linear {
    let burn_g_per_s = expenditure_w(w, b) * w.k("body.metabolism.carb_share_rest") / (w.k("body.metabolism.glycogen_kj_per_g") * 1000.0);
    let cur = m.glycogen_g.at(now).max(0.0);
    Linear { x0: cur, rate: if cur > 0.0 { -burn_g_per_s } else { 0.0 }, t0: now }
}

fn energy(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    let people: Vec<EntityId> = if ev.is("init") || ev.is("midnight") { w.entities_with(w.kind_of::<Metabolism>().unwrap()) } else { vec![ev.subject] };
    for e in people {
        let (Some(m), Some(b)) = (w.get::<Metabolism>(e), w.get::<Body>(e)) else { continue };
        let mut m = m.clone();
        match ev.kind_name {
            "absorbed" => {
                let Some(a) = ev.decode::<Absorbed>() else { continue };
                let cap = w.k("body.metabolism.glycogen_cap_g");
                let g = (m.glycogen_g.at(now) + a.carb_g).min(cap);
                m.glycogen_g = Linear { x0: g, rate: m.glycogen_g.rate, t0: now };
                m.intake_kj_today += a.kj;
            }
            "midnight" => {
                // Hall/Chow: the day's balance moves the equilibrium weight; τ ≈ 1 year
                let spent_kj = expenditure_w(w, b) * 86.4; // W × 86400 s / 1000
                let balance = m.intake_kj_today - spent_kj;
                let per_kg = w.k("body.metabolism.kj_per_day_per_kg");
                let target = (m.fat_kg.at(now) + balance / per_kg).max(1.0);
                m.fat_kg = m.fat_kg.reanchor(now, target, Span::from_days_f64(w.k("body.metabolism.weight_tau_days")));
                m.intake_kj_today = 0.0;
            }
            _ => {}
        }
        m.glycogen_g = reanchor_glycogen(w, b, &m, now);
        out.set(e, m);
    }
    out
}
