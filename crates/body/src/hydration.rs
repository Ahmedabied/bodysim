//! Water: plasma osmolality, thirst, AVP, urine, bladder.
//!
//! Two linear laws from one controlled-infusion study carry the whole subsystem
//! (Thompson & Baylis 1987):
//!   thirst (cm VAS) = 0.3 * (Posm − 281)   ·   plasma AVP (pmol/L) = 0.4 * (Posm − 285)
//! and the bladder is the simplest case of all: constant inflow into constant compliance means
//! `V(t) = V0 + Q·t` and `P = P0 + V/C`, so every sensation threshold is a scheduled event solved
//! in closed form - no polling, no integration.
use crate::intero::{schedule_band_change, Band, Interocept, Sense, EV_INTERO};
use crate::person::Body;
use bodysim_core::component;
use bodysim_core::prelude::*;
use serde::{Deserialize, Serialize};

component! {
    /// Body water. `deficit_l` (litres below the rested total body water; negative = overhydrated)
    /// is the ONE stored state: it accumulates at the net loss rate and steps down at a drink.
    /// Plasma osmolality, thirst, AVP and urine rate are all DERIVED from it (solute is conserved).
    /// `tbw_l` is fixed per body (Watson).
    pub struct Hydration = "hydration" {
        pub deficit_l: Linear,
        pub tbw_l: f64,
        pub thirst_band: Band,
    }
}

/// Plasma osmolality from the water deficit: solute is conserved, so Posm rises with the deficit
/// (linearised; `osm_per_l_deficit` is for a 33 L reference body, scaled by this body's TBW).
pub fn osmolality(w: &World, deficit_l: f64, tbw_l: f64) -> f64 {
    w.k("body.hydration.osmolality_rest_mosm") + w.k("body.hydration.osm_per_l_deficit") * deficit_l * (33.0 / tbw_l.max(1.0))
}
/// The deficit at which osmolality equals `posm` (the inverse, for scheduling thirst bands).
pub fn deficit_for(w: &World, posm: f64, tbw_l: f64) -> f64 {
    (posm - w.k("body.hydration.osmolality_rest_mosm")) / (w.k("body.hydration.osm_per_l_deficit") * (33.0 / tbw_l.max(1.0)))
}

component! {
    /// The bladder: a linear accumulator with its four sensation thresholds.
    pub struct Bladder = "bladder" { pub v: Linear, pub band: Band }
}

pub const EV_VOID: &str = "void";
pub const EV_BALANCE: &str = "water_balance";
pub const WHY_THIRST: &str = "thirst band crossing";
pub const WHY_BLADDER: &str = "bladder sensation threshold";
pub const WHY_BALANCE: &str = "water balance re-anchor";

/// Payload of `void`.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Void { pub by: EntityId }

pub struct HydrationPlugin;

impl Plugin for HydrationPlugin {
    fn name(&self) -> &'static str { "hydration" }
    fn register(&self, r: &mut Registry) {
        r.component::<Hydration>();
        r.component::<Bladder>();
        r.external_event(EV_VOID); // the person decides when: the world or a behaviour layer injects it
        r.event(EV_BALANCE);
        r.event(EV_INTERO);
        r.constants_ron("body.hydration", include_str!("../../../content/constants/body/hydration.ron"));
        r.system(SystemDescriptor {
            name: "water",
            reads: &["hydration", "bladder", "body", "position", "room_air", "thermo"],
            reads_opt: &["sleep"],
            writes: &["hydration", "bladder"],
            handles: &["init", EV_BALANCE, EV_VOID, "drink", EV_INTERO],
            handles_opt: &["sleep_onset", "wake"],
            schedules: &[EV_INTERO, EV_BALANCE],
            consts: &[
                "body.hydration.thirst_threshold_mosm", "body.hydration.thirst_slope_cm_per_mosm",
                "body.hydration.avp_threshold_mosm", "body.hydration.avp_slope_pmol_per_mosm",
                "body.hydration.avp_saturating_pmol", "body.hydration.osmolality_rest_mosm",
                "body.hydration.insensible_skin_ml_per_day_at_22c", "body.hydration.insensible_skin_ml_per_day_at_30c",
                "body.hydration.insensible_resp_ml_per_day", "body.hydration.metabolic_water_ml_per_day",
                "body.hydration.urine_min_ml_per_day", "body.hydration.urine_max_ml_per_hour",
                "body.hydration.osm_per_l_deficit",
                "body.hydration.bladder_first_sensation_ml", "body.hydration.bladder_first_desire_ml",
                "body.hydration.bladder_strong_desire_ml", "body.hydration.bladder_capacity_ml",
                "body.hydration.void_residual_ml", "body.hydration.thirst_band_pressing_cm",
                "body.hydration.thirst_band_urgent_cm", "body.hydration.thirst_band_noticeable_cm",
                "body.hydration.urine_typical_ml_per_day", "body.hydration.osm_full_dilution_mosm",
                "world.thermal.outdoor_c", "body.hydration.nocturnal_gfr_multiplier",
            ],
            handler: water,
            ..Default::default()
        });
        r.rule(DeclRule::Range { kind: "hydration".into(), field: "tbw_l".into(), min: 5.0, max: 120.0 });
        // THE RATCHET LAW, twice: both of this plugin's accumulators name what brings them down.
        r.bounded(BoundedDecl {
            kind: "hydration", field: "deficit_l", min: -3.0, max: 8.0,
            raised_by: &[Mechanism { system: "water", on: EV_BALANCE }],
            lowered_by: &[Mechanism { system: "water", on: "drink" }],
            tau_hint: Some(Span::hours(6)),
            rest: RestBound::Neither,
        });
        r.bounded(BoundedDecl {
            kind: "bladder", field: "v", min: 0.0, max: 1200.0,
            raised_by: &[Mechanism { system: "water", on: EV_BALANCE }],
            lowered_by: &[Mechanism { system: "water", on: EV_VOID }],
            tau_hint: Some(Span::hours(4)),
            rest: RestBound::Neither,
        });
        // If the bladder ever passes maximum cystometric capacity, it must be back below within
        // the hour: someone goes, or something is wrong with the loop. The threshold IS the content's
        // capacity, read once here, so retuning the constant moves the check with it. (A missing row
        // is already a defect at finalize: the water system declares it.)
        let capacity = r.constants().value("body.hydration.bladder_capacity_ml").unwrap_or(f64::NAN);
        r.invariant(Invariant::BoundedResponse {
            name: "a full bladder empties within the hour",
            kind: "bladder", field: "v", threshold: capacity, within: Span::hours(1),
        });
    }
}

/// Thirst on the 0-10 cm visual-analogue scale (Thompson & Baylis).
pub fn thirst_cm(w: &World, posm: f64) -> f64 {
    (w.k("body.hydration.thirst_slope_cm_per_mosm") * (posm - w.k("body.hydration.thirst_threshold_mosm"))).max(0.0)
}

/// Plasma AVP, pmol/L (Thompson & Baylis).
pub fn avp_pmol(w: &World, posm: f64) -> f64 {
    (w.k("body.hydration.avp_slope_pmol_per_mosm") * (posm - w.k("body.hydration.avp_threshold_mosm"))).max(0.0)
}

/// Urine production, mL/s, from plasma osmolality, on three published anchors
/// full dilution (Posm at the bottom of the normal range) → maximum
/// free-water clearance; the AVP threshold → the typical measured output; AVP saturated → the
/// obligatory floor. Below threshold the rise toward maximal clearance is EXPONENTIAL in the
/// shortfall (free-water clearance climbs steeply only as AVP nears full suppression); above it,
/// linear in AVP down to the floor. `night` applies the measured GFR trough multiplier.
pub fn urine_ml_per_s(w: &World, posm: f64, night_multiplier: f64) -> f64 {
    let floor = w.k("body.hydration.urine_min_ml_per_day") / 86_400.0;
    let typical = w.k("body.hydration.urine_typical_ml_per_day") / 86_400.0;
    let max = w.k("body.hydration.urine_max_ml_per_hour") / 3600.0;
    let lo = w.k("body.hydration.osm_full_dilution_mosm");
    let thr = w.k("body.hydration.avp_threshold_mosm");
    let sat_avp = w.k("body.hydration.avp_saturating_pmol");
    let slope = w.k("body.hydration.avp_slope_pmol_per_mosm");
    let hi = thr + sat_avp / slope;
    let rate = if posm <= lo { max }
        else if posm < thr { typical * (max / typical).powf((thr - posm) / (thr - lo)) }
        else if posm < hi { typical + (floor - typical) * (posm - thr) / (hi - thr) }
        else { floor };
    rate * night_multiplier
}

/// Insensible skin loss, mL/s, interpolated on ambient temperature between the two measured points.
pub fn insensible_ml_per_s(w: &World, ambient_c: f64, bsa_m2: f64) -> f64 {
    let (a22, a30) = (w.k("body.hydration.insensible_skin_ml_per_day_at_22c"), w.k("body.hydration.insensible_skin_ml_per_day_at_30c"));
    let skin = a22 + (a30 - a22) * ((ambient_c - 22.0) / 8.0);
    // the measurements are for a 1.75 m² body
    let skin = skin.max(0.0) * bsa_m2 / 1.75;
    (skin + w.k("body.hydration.insensible_resp_ml_per_day")) / 86_400.0
}

fn ambient_c(w: &World, e: EntityId, now: SimTime) -> f64 {
    world_ambient(w, e, now)
}

/// The one place the body reads the world's air temperature: through `position` → `room_air`,
/// exactly as the world publishes it. (Duplicated here rather than depending on `the world crate-world`:
/// body and world never depend on each other - they meet through components and events.)
fn world_ambient(w: &World, e: EntityId, now: SimTime) -> f64 {
    let outdoor = w.k("world.thermal.outdoor_c");
    let Some(room) = w.field_named(e, "position", "room").and_then(|v| match v { Value::Id(r) => Some(r), _ => None }) else { return outdoor };
    match w.field_named(room, "room_air", "t") {
        Some(Value::Relax(r)) => r.at(now),
        _ => outdoor,
    }
}

fn water(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    if ev.is("init") {
        for e in w.entities_with(w.kind_of::<Hydration>().unwrap()) {
            out.schedule_at(now, EV_BALANCE, e, WHY_BALANCE);
        }
        return out;
    }
    // Whose body is this event about?
    let person = match ev.kind_name {
        "drink" => match ev.decode::<crate::DrinkPayload>() { Some(d) => d.by, None => return out },
        "void" => match ev.decode::<Void>() { Some(v) => v.by, None => ev.subject },
        _ => ev.subject,
    };
    let (Some(hyd), Some(bl), Some(body)) = (w.get::<Hydration>(person), w.get::<Bladder>(person), w.get::<Body>(person)) else { return out };
    let mut hyd = hyd.clone();
    let mut bl = bl.clone();

    match ev.kind_name {
        "drink" => {
            let Some(d) = ev.decode::<crate::DrinkPayload>() else { return out };
            // water in: the deficit steps down now. Thirst relief is fast in life too - oropharyngeal
            // signals precede absorption - so an instant step is the simplest honest model.
            hyd.deficit_l = hyd.deficit_l.reset(now, hyd.deficit_l.at(now) - d.volume_l);
        }
        "void" => {
            bl.v = bl.v.reset(now, w.k("body.hydration.void_residual_ml"));
        }
        _ => {}
    }

    // --- re-anchor the water balance for the conditions right now ---
    let amb = ambient_c(w, person, now);
    let insensible = insensible_ml_per_s(w, amb, body.bsa_m2);
    let metabolic = w.k("body.hydration.metabolic_water_ml_per_day") / 86_400.0;
    let sweat = match w.field_named(person, "thermo", "sweat_g_per_s") { Some(Value::F64(g)) => g, _ => 0.0 };
    let posm = osmolality(w, hyd.deficit_l.at(now), hyd.tbw_l);
    // asleep: the measured nocturnal GFR trough (and the AVP night rise) - a multiplier that is
    // REMOVED at the next re-anchor after waking, never latched
    let asleep = matches!(w.field_named(person, "sleep", "state"), Some(Value::Enum(1)));
    let night = if asleep { w.k("body.hydration.nocturnal_gfr_multiplier") } else { 1.0 };
    let urine = urine_ml_per_s(w, posm, night);
    let net_loss_ml_per_s = insensible + urine + sweat - metabolic;
    hyd.deficit_l = hyd.deficit_l.reanchor(now, net_loss_ml_per_s / 1000.0);
    bl.v = bl.v.reanchor(now, urine);

    // --- bands (the only thing that reaches the mind) ---
    let (bn, bp, bu) = (
        w.k("body.hydration.bladder_first_desire_ml"),
        w.k("body.hydration.bladder_strong_desire_ml"),
        w.k("body.hydration.bladder_capacity_ml"),
    );
    let (tn, tp, tu) = (w.k("body.hydration.thirst_band_noticeable_cm"), w.k("body.hydration.thirst_band_pressing_cm"), w.k("body.hydration.thirst_band_urgent_cm"));
    let thirst = thirst_cm(w, osmolality(w, hyd.deficit_l.at(now), hyd.tbw_l));
    let mut new_thirst_band = Band::of(thirst, tn, tp, tu);
    let mut new_bladder_band = Band::of(bl.v.at(now), bn, bp, bu);
    // a crossing we scheduled ourselves has landed: its payload is the truth at this instant
    if ev.is(EV_INTERO) {
        if let Some(i) = ev.decode::<Interocept>() {
            match i.sense { Sense::Thirst => new_thirst_band = i.band, Sense::Bladder => new_bladder_band = i.band, _ => {} }
        }
    }

    // a band that moved by a STEP (a drink, a void) is announced now; a scheduled crossing
    // already is the announcement
    // an act on a need is always answered: after a drink she feels whether the thirst went
    let acted_thirst = ev.is("drink");
    let acted_bladder = ev.is(EV_VOID);
    if new_thirst_band != hyd.thirst_band || acted_thirst {
        if !ev.is(EV_INTERO) {
            out.schedule_with(now, EV_INTERO, person, WHY_THIRST,
                &Interocept { sense: Sense::Thirst, band: new_thirst_band, rising: new_thirst_band.rank() > hyd.thirst_band.rank() });
        }
        hyd.thirst_band = new_thirst_band;
    }
    if new_bladder_band != bl.band || acted_bladder {
        if !ev.is(EV_INTERO) {
            out.schedule_with(now, EV_INTERO, person, WHY_BLADDER,
                &Interocept { sense: Sense::Bladder, band: new_bladder_band, rising: new_bladder_band.rank() > bl.band.rank() });
        }
        bl.band = new_bladder_band;
    }

    // --- when does the next band boundary arrive? solved, not polled ---
    out.cancel_pending_why(person, EV_INTERO, WHY_BLADDER);
    out.cancel_pending_why(person, EV_INTERO, WHY_THIRST);
    schedule_band_change(&mut out, now, &bl.v, bl.band, Sense::Bladder, bn, bp, bu, person, WHY_BLADDER);
    // thirst is linear in osmolality, which is linear in the deficit: a thirst band is a deficit threshold
    let slope = w.k("body.hydration.thirst_slope_cm_per_mosm");
    let thr = w.k("body.hydration.thirst_threshold_mosm");
    let to_deficit = |cm: f64| deficit_for(w, thr + cm / slope, hyd.tbw_l);
    schedule_band_change(&mut out, now, &hyd.deficit_l, hyd.thirst_band, Sense::Thirst, to_deficit(tn), to_deficit(tp), to_deficit(tu), person, WHY_THIRST);
    // and a bounded re-anchor so a settled body still re-checks its balance
    if !ev.is(EV_INTERO) {
        out.cancel_pending(person, EV_BALANCE);
        out.schedule_in(now, Span::hours(1), EV_BALANCE, person, WHY_BALANCE);
    }

    out.set(person, hyd);
    out.set(person, bl);
    out
}
