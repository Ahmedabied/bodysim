//! Pharmacokinetics, caffeine first: every dose is a Bateman curve (the
//! fitted population example: ka 0.064 /min, V/F 38.6 L, t½ 5.3 h); plasma concentration is the
//! SUM of live doses (derived); receptor occupancy is an Emax on it. Doses decay to nothing and
//! are pruned - a self-decaying ratchet.
use bodysim_core::component;
use bodysim_core::prelude::*;

component! {
    pub struct Caffeine = "caffeine" { pub doses: Vec<Bateman> }
}

pub const EV_DOSE: &str = "dose";

pub struct PkPlugin;

impl Plugin for PkPlugin {
    fn name(&self) -> &'static str { "pk" }
    fn register(&self, r: &mut Registry) {
        r.component::<Caffeine>();
        r.external_event(EV_DOSE);
        r.constants_ron("body.pk", include_str!("../../../content/constants/body/pk.ron"));
        r.system(SystemDescriptor {
            name: "pharmacokinetics",
            reads: &["caffeine"],
            writes: &["caffeine"],
            handles: &[EV_DOSE],
            consts: &["body.pk.caffeine_ka_per_min", "body.pk.caffeine_half_life_h", "body.pk.caffeine_vd_l", "body.pk.caffeine_f"],
            handler: pharmacokinetics,
            ..Default::default()
        });
    }
}

/// Plasma caffeine, mg/L.
pub fn caffeine_mg_l(c: &Caffeine, t: SimTime) -> f64 { c.doses.iter().map(|d| d.at(t)).sum() }
/// Fractional adenosine-receptor occupancy (Emax, EC50 from content).
pub fn caffeine_occupancy(w: &World, e: EntityId, t: SimTime) -> f64 {
    let Some(c) = w.get::<Caffeine>(e) else { return 0.0 };
    let conc = caffeine_mg_l(c, t);
    let ec50 = w.k("body.pk.caffeine_ec50_mg_l");
    conc / (conc + ec50)
}

fn pharmacokinetics(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    let e = ev.subject;
    let Some(c) = w.get::<Caffeine>(e) else { return out };
    let mg: f64 = ev.decode().unwrap_or(0.0);
    // A dose comes from outside, and the engine's finite check cannot see inside a list of doses:
    // one NaN or infinite amount would make the sleep onset threshold NaN for good.
    if !mg.is_finite() || mg < 0.0 {
        out.note(format!("dose refused: {mg} mg is not a finite amount of zero or more"));
        return out;
    }
    if mg == 0.0 { return out; }
    let mut c = c.clone();
    let ke = (2f64).ln() / (w.k("body.pk.caffeine_half_life_h") * 3600.0);
    c.doses.push(Bateman { dose: mg, f: w.k("body.pk.caffeine_f"), ka: w.k("body.pk.caffeine_ka_per_min") / 60.0, ke, vd: w.k("body.pk.caffeine_vd_l"), t0: now });
    let horizon = Span::from_hours_f64(6.0 * w.k("body.pk.caffeine_half_life_h"));
    c.doses.retain(|d| now - d.t0 < horizon);
    out.set(e, c);
    out
}
