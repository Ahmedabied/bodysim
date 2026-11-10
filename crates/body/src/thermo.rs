//! Thermoregulation: the Gagge two-node model run as CLOSED-FORM SEGMENTS between events
//! ("the single most valuable structural finding in this report").
//!
//! Between events the control regime (skin blood flow, sweat rate) is held, and core/skin
//! temperature obey an exact linear 2-state ODE - an `Expm2`. A segment ends when a control
//! signal would have moved (the state drifts past a re-linearisation bound), when the
//! environment changes, or after a bounded span; then the regime is recomputed from Gagge's own
//! equations (verbatim from pythermalcomfort) and a new segment starts.
use crate::intero::{Band, Interocept, Sense, EV_INTERO};
use crate::person::Body;
use bodysim_core::component;
use bodysim_core::prelude::*;

component! {
    /// Core and skin temperature as one coupled anchor: `state` = [Tsk, Tcr].
    pub struct Thermo = "thermo" {
        pub state: Expm2,
        pub skbf: f64,
        pub sweat_g_per_s: f64,
        pub hot_band: Band,
        pub cold_band: Band,
    }
}

pub const EV_SEGMENT: &str = "thermo_segment";
pub const WHY_SEGMENT: &str = "thermoregulation re-linearisation";
pub const WHY_HOT: &str = "thermal band crossing";

pub struct ThermoPlugin;

impl Plugin for ThermoPlugin {
    fn name(&self) -> &'static str { "thermo" }
    fn register(&self, r: &mut Registry) {
        r.component::<Thermo>();
        r.event(EV_SEGMENT);
        r.constants_ron("body.thermo", include_str!("../../../content/constants/body/thermo.ron"));
        r.system(SystemDescriptor {
            name: "thermoreg",
            reads: &["thermo", "body", "position", "room_air"],
            reads_opt: &["cycle", "garment"],
            writes: &["thermo"],
            handles: &["init", EV_SEGMENT],
            schedules: &[EV_SEGMENT, EV_INTERO],
            consts_opt: &["body.cycle.bbt_shift_c", "body.cycle.menses_d", "body.cycle.ovulatory_window_d"],
            consts: &[
                "body.thermo.tsk_neutral_c", "body.thermo.tcr_neutral_c", "body.thermo.tissue_conductance_w_per_m2k",
                "body.thermo.blood_capacity_wh_per_l_k", "body.thermo.skbf_neutral_kg_per_h_m2", "body.thermo.skbf_max_kg_per_h_m2",
                "body.thermo.skbf_min_kg_per_h_m2", "body.thermo.c_dil", "body.thermo.c_str", "body.thermo.c_sw",
                "body.thermo.sweat_exp_divisor_c", "body.thermo.sweat_max_g_per_h_m2", "body.thermo.sweat_heat_wh_per_g",
                "body.thermo.skin_mass_fraction", "body.thermo.body_heat_capacity_wh_per_kg_k", "body.thermo.met_w_per_m2",
                "body.thermo.respiratory_heat_fraction", "body.thermo.clo_air_layer_m2k_per_w", "body.thermo.clo_unit_m2k_per_w",
                "body.thermo.reanchor_dtsk_c", "body.thermo.reanchor_dtcr_c", "body.thermo.reanchor_max_span_s",
                "body.thermo.warm_band_noticeable_c", "body.thermo.warm_band_pressing_c", "body.thermo.warm_band_urgent_c",
                "body.hydration.insensible_skin_ml_per_day_at_22c", "world.thermal.outdoor_c",
            ],
            handler: thermoreg,
            ..Default::default()
        });
        r.bounded(BoundedDecl {
            kind: "thermo", field: "sweat_g_per_s", min: 0.0, max: 1.0,
            raised_by: &[Mechanism { system: "thermoreg", on: EV_SEGMENT }],
            lowered_by: &[Mechanism { system: "thermoreg", on: EV_SEGMENT }],
            tau_hint: Some(Span::mins(30)),
            rest: RestBound::Min,
        });
        r.bounded(BoundedDecl {
            kind: "thermo", field: "skbf", min: 0.5, max: 90.0,
            raised_by: &[Mechanism { system: "thermoreg", on: EV_SEGMENT }],
            lowered_by: &[Mechanism { system: "thermoreg", on: EV_SEGMENT }],
            tau_hint: Some(Span::mins(30)),
            rest: RestBound::Neither,
        });
        // Core and skin temperature live inside the coupled anchor; a typed rule keeps them human.
        r.rule_with(|k| {
            let kind = k.component("thermo").ok_or("thermo: no kind")?;
            let f = k.field(kind, "state").ok_or("thermo: no state")?;
            Ok(Box::new(Alive { kind, sig: Signature::from_kinds([kind]), touches: vec![(kind, f)] }))
        });
    }
}

/// Hypothermia <35 °C and heat stroke >40 °C core (NBK499843) are not
/// refused - they are real states - but a temperature outside what a living body can hold is a bug.
struct Alive { kind: bodysim_core::component::ComponentKind, sig: Signature, touches: Vec<(bodysim_core::component::ComponentKind, FieldId)> }
impl Rule for Alive {
    fn name(&self) -> &str { "thermo: core 25-45 °C, skin 10-45 °C" }
    fn signature(&self) -> Signature { self.sig }
    fn touches(&self) -> &[(bodysim_core::component::ComponentKind, FieldId)] { &self.touches }
    fn check(&self, w: &World, v: &RuleView) -> Result<(), String> {
        if v.component != Some(self.kind) { return Ok(()); }
        let Some(t) = v.new.and_then(|c| bodysim_core::component::downcast::<Thermo>(c)) else { return Ok(()) };
        let [tsk, tcr] = t.state.state_at(w.now);
        if !(25.0..=45.0).contains(&tcr) { return Err(format!("core {tcr:.2} °C")); }
        if !(10.0..=45.0).contains(&tsk) { return Err(format!("skin {tsk:.2} °C")); }
        Ok(())
    }
}

/// Gagge's control equations for one regime, evaluated at the segment start (verbatim constants).
struct Regime { skbf: f64, m_rsw_g_per_h_m2: f64 }

fn regime(w: &World, e: EntityId, now: SimTime, tsk: f64, tcr: f64) -> Regime {
    // the luteal phase raises the core set point (+0.3-0.7 °C)
    let shift = match w.get::<crate::cycle::Cycle>(e) { Some(c) => crate::cycle::bbt_shift_c(w, c, now), None => 0.0 };
    let (tskn, tcrn) = (w.k("body.thermo.tsk_neutral_c"), w.k("body.thermo.tcr_neutral_c") + shift);
    let warm_sk = (tsk - tskn).max(0.0);
    let cold_sk = (tskn - tsk).max(0.0);
    let warm_cr = (tcr - tcrn).max(0.0);
    let skbf = ((w.k("body.thermo.skbf_neutral_kg_per_h_m2") + w.k("body.thermo.c_dil") * warm_cr) / (1.0 + w.k("body.thermo.c_str") * cold_sk))
        .clamp(w.k("body.thermo.skbf_min_kg_per_h_m2"), w.k("body.thermo.skbf_max_kg_per_h_m2"));
    let alpha = w.k("body.thermo.skin_mass_fraction");
    let warm_body = alpha * warm_sk + (1.0 - alpha) * warm_cr;
    let m_rsw = (w.k("body.thermo.c_sw") * warm_body * (warm_sk / w.k("body.thermo.sweat_exp_divisor_c")).exp()).min(w.k("body.thermo.sweat_max_g_per_h_m2"));
    Regime { skbf, m_rsw_g_per_h_m2: m_rsw }
}

/// The LTI segment for a held regime: d/dt [Tsk; Tcr] = A·x + u, in °C/s.
/// Insulation from the garments actually worn (the world's `garment` kind and `wearing` relation,
/// read by name - body never depends on world), with pythermalcomfort's ensemble correction.
fn worn_clo(w: &World, e: EntityId) -> Option<f64> {
    let rk = w.kinds.relation("wearing")?;
    let clos: Vec<f64> = w.edges.open(e, rk).iter().filter(|x| x.src == e)
        .filter_map(|x| match w.field_named(x.dst, "garment", "clo") { Some(Value::F64(c)) => Some(c), _ => None }).collect();
    if clos.is_empty() { None } else { Some(clos.iter().sum::<f64>() * 0.835 + 0.161) }
}

fn segment(w: &World, e: EntityId, body: &Body, ambient_c: f64, reg: &Regime, x0: [f64; 2], now: SimTime) -> Expm2 {
    let alpha = w.k("body.thermo.skin_mass_fraction");
    let cap = w.k("body.thermo.body_heat_capacity_wh_per_kg_k");
    let csk_wh = cap * alpha * body.mass_kg;
    let ccr_wh = cap * (1.0 - alpha) * body.mass_kg;
    let bsa = body.bsa_m2;
    let ksk = bsa / (csk_wh * 3600.0); // (W/m²) → °C/s
    let kcr = bsa / (ccr_wh * 3600.0);
    let k_cs = w.k("body.thermo.tissue_conductance_w_per_m2k") + w.k("body.thermo.blood_capacity_wh_per_l_k") * reg.skbf;
    let clo = worn_clo(w, e).unwrap_or(body.clo);
    let r_env = clo * w.k("body.thermo.clo_unit_m2k_per_w") + w.k("body.thermo.clo_air_layer_m2k_per_w");
    let e_rsw = w.k("body.thermo.sweat_heat_wh_per_g") * reg.m_rsw_g_per_h_m2; // W/m²
    // insensible diffusion, from the measured skin water loss (hydration's constant), per m² of a 1.75 m² body
    let e_diff = w.k("body.hydration.insensible_skin_ml_per_day_at_22c") / 86_400.0 * (w.k("body.thermo.sweat_heat_wh_per_g") * 3600.0) / 1.75;
    let m = w.k("body.thermo.met_w_per_m2") * body.met();
    let m_net = m * (1.0 - w.k("body.thermo.respiratory_heat_fraction"));
    let a = [[-(k_cs + 1.0 / r_env) * ksk, k_cs * ksk], [k_cs * kcr, -k_cs * kcr]];
    let u = [(ambient_c / r_env - e_rsw - e_diff) * ksk, m_net * kcr];
    Expm2 { a, u, x0, t0: now }
}

fn ambient_c(w: &World, e: EntityId, now: SimTime) -> f64 {
    let outdoor = w.k("world.thermal.outdoor_c");
    let Some(room) = w.field_named(e, "position", "room").and_then(|v| match v { Value::Id(r) => Some(r), _ => None }) else { return outdoor };
    match w.field_named(room, "room_air", "t") { Some(Value::Relax(r)) => r.at(now), _ => outdoor }
}

fn thermoreg(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    if ev.is("init") {
        for e in w.entities_with(w.kind_of::<Thermo>().unwrap()) { out.schedule_at(now, EV_SEGMENT, e, WHY_SEGMENT); }
        return out;
    }
    let e = ev.subject;
    let (Some(th), Some(body)) = (w.get::<Thermo>(e), w.get::<Body>(e)) else { return out };
    let [tsk, tcr] = th.state.state_at(now);
    let reg = regime(w, e, now, tsk, tcr);
    let amb = ambient_c(w, e, now);
    let seg = segment(w, e, body, amb, &reg, [tsk, tcr], now);
    let sweat_g_per_s = reg.m_rsw_g_per_h_m2 * body.bsa_m2 / 3600.0;

    // bands from skin excess over neutral (alliesthesia gives the sign; thresholds are Tunable)
    let tskn = w.k("body.thermo.tsk_neutral_c");
    let (n, p, u) = (w.k("body.thermo.warm_band_noticeable_c"), w.k("body.thermo.warm_band_pressing_c"), w.k("body.thermo.warm_band_urgent_c"));
    let hot = Band::of(tsk - tskn, n, p, u);
    let cold = Band::of(tskn - tsk, n, p, u);
    if hot != th.hot_band {
        out.schedule_with(now, EV_INTERO, e, WHY_HOT, &Interocept { sense: Sense::Hot, band: hot, rising: hot.rank() > th.hot_band.rank() });
    }
    if cold != th.cold_band {
        out.schedule_with(now, EV_INTERO, e, WHY_HOT, &Interocept { sense: Sense::Cold, band: cold, rising: cold.rank() > th.cold_band.rank() });
    }

    // when must this segment end? whichever comes first: skin drift, core drift, the span bound,
    // or the next thermal band boundary - every one solved on the closed form.
    let horizon = Span::from_secs_f64(w.k("body.thermo.reanchor_max_span_s"));
    let (dsk, dcr) = (w.k("body.thermo.reanchor_dtsk_c"), w.k("body.thermo.reanchor_dtcr_c"));
    let mut asks = vec![(0, tsk + dsk, true), (0, tsk - dsk, false), (1, tcr + dcr, true), (1, tcr - dcr, false)];
    for thr in [n, p, u] {
        asks.extend([(0, tskn + thr, true), (0, tskn + thr, false), (0, tskn - thr, true), (0, tskn - thr, false)]);
    }
    // sixteen thresholds on one trajectory: sampled once for all of them (this handler was three quarters of a run)
    let next = seg.first_reach(&asks, horizon).unwrap_or(now + horizon);
    out.cancel_pending(e, EV_SEGMENT);
    out.schedule_at(next.max(now + Span::secs(1)), EV_SEGMENT, e, WHY_SEGMENT);
    out.set(e, Thermo { state: seg, skbf: reg.skbf, sweat_g_per_s, hot_band: hot, cold_band: cold });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::intero::Band;
    use bodysim_core::content::from_ron;
    #[test]
    fn thermo_parses_from_content_ron() {
        let t: Result<Thermo, String> = from_ron(r#"(state: (a: ((0.0, 0.0), (0.0, 0.0)), u: (0.0, 0.0), x0: (33.7, 36.8), t0: 0), skbf: 6.3, sweat_g_per_s: 0.0, hot_band: None, cold_band: None)"#);
        assert!(t.is_ok(), "{t:?}");
        assert_eq!(t.unwrap().hot_band, Band::None);
        let b: Result<Band, String> = from_ron("Pressing");
        assert_eq!(b, Ok(Band::Pressing));
        let e: Result<Expm2, String> = from_ron("(a: ((0.0, 0.0), (0.0, 0.0)), u: (0.0, 0.0), x0: (33.7, 36.8), t0: 0)");
        assert!(e.is_ok(), "{e:?}");
    }
}
