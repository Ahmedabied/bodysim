//! The gut. Gastric fill is Elashoff's power-exponential retention (
//! "the validated model IS a formula"); ghrelin is a `Relax` with a SCHEDULED anticipatory rise
//! before each habitual meal, RE-ARMED every day (the one-shot ghrelin bug, refused by design);
//! hunger is DERIVED; the colon is a `Linear` with balloon-manometry thresholds.
//! While a meal empties, a bounded episode of `absorbed` events feeds metabolism and glucose.
use crate::intero::{schedule_band_change, Band, Interocept, Sense, EV_INTERO};
use crate::metabolism::{Absorbed, EV_ABSORBED};
use bodysim_core::relax::first_crossing_scan;
use bodysim_core::{component, value_enum};
use bodysim_core::prelude::*;
use serde::{Deserialize, Serialize};

value_enum! {
    pub enum MealKind { Liquid, Solid }
}

/// A meal in the stomach: Elashoff anchor.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Meal { pub t0: SimTime, pub mass_g: f64, pub carb_g: f64, pub kj: f64, pub k_per_min: f64, pub beta: f64, pub absorbed_frac: f64 }

impl Meal {
    /// Fractional retention at `t` (Elashoff modified power-exponential).
    pub fn retained(&self, t: SimTime) -> f64 {
        if t <= self.t0 { return 1.0; }
        let m = (t - self.t0).as_mins_f64();
        1.0 - (1.0 - (-self.k_per_min * m).exp()).powf(self.beta)
    }
    pub fn in_stomach_g(&self, t: SimTime) -> f64 { self.mass_g * self.retained(t) }
}

component! {
    pub struct Digestion = "digestion" {
        pub meals: Vec<Meal>,
        pub ghrelin: Relax,
        pub colon_ml: Linear,
        pub habitual_meal_hours: Vec<f64>,
        pub hunger_band: Band,
        pub bowel_band: Band,
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Eat { pub mass_g: f64, pub carb_g: f64, pub kj: f64, pub kind: MealKind }

pub const EV_EAT: &str = "eat";
pub const EV_DEFECATE: &str = "defecate";
pub const EV_ANTICIPATE: &str = "meal_anticipation";
pub const EV_GASTRIC: &str = "gastric_check";
pub const WHY_HUNGER: &str = "hunger band crossing";
pub const WHY_BOWEL: &str = "bowel band crossing";

pub struct DigestionPlugin;

impl Plugin for DigestionPlugin {
    fn name(&self) -> &'static str { "digestion" }
    fn register(&self, r: &mut Registry) {
        r.component::<Digestion>();
        r.external_event(EV_EAT);
        r.external_event(EV_DEFECATE);
        r.event(EV_ANTICIPATE);
        r.event(EV_GASTRIC);
        r.constants_ron("body.digestion", include_str!("../../../content/constants/body/digestion.ron"));
        r.system(SystemDescriptor {
            name: "gut",
            reads: &["digestion"],
            reads_opt: &["clock"],
            writes: &["digestion"],
            handles: &["init", EV_EAT, EV_DEFECATE, EV_ANTICIPATE, EV_GASTRIC, EV_INTERO],
            schedules: &[EV_ANTICIPATE, EV_GASTRIC, EV_ABSORBED, EV_INTERO],
            consts: &["body.digestion.k_liquid_per_min", "body.digestion.beta_liquid", "body.digestion.k_solid_per_min", "body.digestion.beta_solid",
                      "body.digestion.ghrelin_baseline", "body.digestion.ghrelin_peak", "body.digestion.ghrelin_tau_rise_s", "body.digestion.ghrelin_tau_fall_s",
                      "body.digestion.anticipation_lead_s", "body.digestion.fullness_weight", "body.digestion.gastric_max_ml",
                      "body.digestion.hunger_noticeable", "body.digestion.hunger_pressing", "body.digestion.hunger_urgent",
                      "body.digestion.colon_first_ml", "body.digestion.colon_urge_ml", "body.digestion.colon_max_ml", "body.digestion.colon_fill_ml_per_s",
                      "body.digestion.gastric_check_s"],
            handler: gut,
            ..Default::default()
        });
        r.bounded(BoundedDecl {
            kind: "digestion", field: "ghrelin", min: 0.0, max: 2.0,
            raised_by: &[Mechanism { system: "gut", on: EV_ANTICIPATE }],
            lowered_by: &[Mechanism { system: "gut", on: EV_EAT }],
            tau_hint: Some(Span::hours(3)), rest: RestBound::Neither,
        });
        r.bounded(BoundedDecl {
            kind: "digestion", field: "colon_ml", min: 0.0, max: 400.0,
            raised_by: &[Mechanism { system: "gut", on: "init" }],
            lowered_by: &[Mechanism { system: "gut", on: EV_DEFECATE }],
            tau_hint: Some(Span::hours(30)), rest: RestBound::Neither,
        });
    }
}

/// Grams in the stomach now (all meals).
pub fn gastric_g(d: &Digestion, t: SimTime) -> f64 { d.meals.iter().map(|m| m.in_stomach_g(t)).sum() }
/// Hunger drive ∈ [0, ~1]: ghrelin above baseline minus the fullness term.
pub fn hunger(w: &World, d: &Digestion, t: SimTime) -> f64 {
    hunger_with(d, t, w.k("body.digestion.gastric_max_ml"), w.k("body.digestion.fullness_weight"))
}
/// `hunger` with its two constants already looked up: a crossing search asks it over a hundred times.
fn hunger_with(d: &Digestion, t: SimTime, gastric_max_ml: f64, fullness_weight: f64) -> f64 {
    let g = d.ghrelin.at(t);
    let full = gastric_g(d, t) / gastric_max_ml;
    (g - fullness_weight * full).max(0.0)
}

fn tod_hours(w: &World, t: SimTime) -> f64 { crate::circadian::tod_hours(w, t) }

/// Next habitual-meal anticipation time strictly after `now`.
fn next_anticipation(w: &World, d: &Digestion, now: SimTime) -> Option<SimTime> {
    let lead = w.k("body.digestion.anticipation_lead_s") / 3600.0;
    let tod = tod_hours(w, now);
    let mut best: Option<f64> = None;
    for h in &d.habitual_meal_hours {
        let mut dt = (h - lead) - tod;
        if dt <= 0.001 { dt += 24.0; }
        if best.is_none_or(|b| dt < b) { best = Some(dt); }
    }
    best.map(|dt| now + Span::from_hours_f64(dt))
}

fn gut(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    if ev.is("init") {
        for e in w.entities_with(w.kind_of::<Digestion>().unwrap()) {
            out.schedule_at(now, EV_GASTRIC, e, "gut schedule at start");
        }
        return out;
    }
    let e = ev.subject;
    let Some(d) = w.get::<Digestion>(e) else { return out };
    let mut d = d.clone();
    let base = w.k("body.digestion.ghrelin_baseline");
    match ev.kind_name {
        "eat" => {
            let Some(m) = ev.decode::<Eat>() else { return out };
            let (k, beta) = match m.kind {
                MealKind::Liquid => (w.k("body.digestion.k_liquid_per_min"), w.k("body.digestion.beta_liquid")),
                MealKind::Solid => (w.k("body.digestion.k_solid_per_min"), w.k("body.digestion.beta_solid")),
            };
            d.meals.push(Meal { t0: now, mass_g: m.mass_g, carb_g: m.carb_g, kj: m.kj, k_per_min: k, beta, absorbed_frac: 0.0 });
            // ghrelin collapses within ~1 h of eating
            d.ghrelin = d.ghrelin.reanchor(now, base * 0.5, Span::from_secs_f64(w.k("body.digestion.ghrelin_tau_fall_s")));
        }
        "defecate" => { d.colon_ml = d.colon_ml.reset(now, 0.0); }
        "meal_anticipation" => {
            // the pre-meal rise: ~2× baseline, then it falls back on its own if no meal comes
            d.ghrelin = d.ghrelin.reanchor(now, w.k("body.digestion.ghrelin_peak"), Span::from_secs_f64(w.k("body.digestion.ghrelin_tau_rise_s")));
        }
        "gastric_check" => {}
        "intero" => {
            if let Some(i) = ev.decode::<Interocept>() {
                match i.sense { Sense::Hunger => d.hunger_band = i.band, Sense::Bowel => d.bowel_band = i.band, _ => return out }
            } else { return out; }
        }
        _ => return out,
    }
    // --- the emptying episode: what left the stomach since the last check reaches the blood ---
    let mut carb = 0.0;
    let mut kj = 0.0;
    for m in &mut d.meals {
        let emptied = 1.0 - m.retained(now);
        let new = (emptied - m.absorbed_frac).max(0.0);
        carb += new * m.carb_g;
        kj += new * m.kj;
        m.absorbed_frac = emptied;
    }
    if carb > 0.0 || kj > 0.0 { out.schedule_with(now, EV_ABSORBED, e, "meal emptying reaches the blood", &Absorbed { carb_g: carb, kj }); }
    // colon: the emptied mass arrives (grossly) as stool volume; a constant fill rate stands in
    d.meals.retain(|m| m.retained(now) > 0.01);
    if d.meals.is_empty() {
        // ghrelin drifts back to baseline once the stomach is empty
        if (d.ghrelin.x_inf - base).abs() > 1e-9 && d.ghrelin.x_inf < base { d.ghrelin = d.ghrelin.reanchor(now, base, Span::from_secs_f64(w.k("body.digestion.ghrelin_tau_rise_s"))); }
    } else {
        out.cancel_pending(e, EV_GASTRIC);
        out.schedule_in(now, Span::from_secs_f64(w.k("body.digestion.gastric_check_s")), EV_GASTRIC, e, "gastric emptying check");
    }
    d.colon_ml = d.colon_ml.reanchor(now, w.k("body.digestion.colon_fill_ml_per_s"));
    // --- anticipation, re-armed every time ---
    if !ev.is("intero") {
        out.cancel_pending(e, EV_ANTICIPATE);
        if let Some(t) = next_anticipation(w, &d, now) { out.schedule_at(t, EV_ANTICIPATE, e, "habitual mealtime approaches"); }
    }
    // --- bands ---
    let (hn, hp, hu) = (w.k("body.digestion.hunger_noticeable"), w.k("body.digestion.hunger_pressing"), w.k("body.digestion.hunger_urgent"));
    let (bn, bp, bu) = (w.k("body.digestion.colon_first_ml"), w.k("body.digestion.colon_urge_ml"), w.k("body.digestion.colon_max_ml"));
    let hb = Band::of(hunger(w, &d, now), hn, hp, hu);
    let bb = Band::of(d.colon_ml.at(now), bn, bp, bu);
    if (hb != d.hunger_band || ev.is("eat")) && !ev.is("intero") {
        out.schedule_with(now, EV_INTERO, e, WHY_HUNGER, &Interocept { sense: Sense::Hunger, band: hb, rising: hb.rank() > d.hunger_band.rank() });
    }
    if (bb != d.bowel_band || ev.is("defecate")) && !ev.is("intero") {
        out.schedule_with(now, EV_INTERO, e, WHY_BOWEL, &Interocept { sense: Sense::Bowel, band: bb, rising: bb.rank() > d.bowel_band.rank() });
    }
    d.hunger_band = hb;
    d.bowel_band = bb;
    // next crossings: bowel is a Linear (exact); hunger is a composite (scan + bisect within 8 h)
    out.cancel_pending_why(e, EV_INTERO, WHY_BOWEL);
    out.cancel_pending_why(e, EV_INTERO, WHY_HUNGER);
    schedule_band_change(&mut out, now, &d.colon_ml, d.bowel_band, Sense::Bowel, bn, bp, bu, e, WHY_BOWEL);
    {
        let rising = d.ghrelin.limit() > d.ghrelin.at(now) || !d.meals.is_empty();
        let target = if rising { hb.next_threshold(hn, hp, hu) } else { hb.prev_threshold(hn, hp, hu) };
        if let Some(tg) = target {
            let sign = if rising { 1.0 } else { -1.0 };
            let (gmax, fw) = (w.k("body.digestion.gastric_max_ml"), w.k("body.digestion.fullness_weight"));
            let f = |s: f64| sign * (hunger_with(&d, now + Span::from_secs_f64(s), gmax, fw) - tg);
            if f(0.0) < 0.0 {
                if let Some(s) = first_crossing_scan(f, 8.0 * 3600.0, 96) {
                    let band_then = if rising { Band::of(tg, hn, hp, hu) } else { Band::of(tg - 1e-3, hn, hp, hu) };
                    out.schedule_with(now + Span::from_secs_f64(s).max(Span::secs(1)), EV_INTERO, e, WHY_HUNGER, &Interocept { sense: Sense::Hunger, band: band_then, rising });
                }
            }
        }
    }
    out.set(e, d);
    out
}
