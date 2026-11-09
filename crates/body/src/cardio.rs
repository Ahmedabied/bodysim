//! Cardio-autonomic: sympathetic and vagal tone as two first-order relaxations toward targets set
//! by events ("the single most useful finding for the engine"); HR is
//! DERIVED: intrinsic − k_v·vagal + k_s·symp. Targets come from activity and, later, from psyche
//! (the DOWN channel, goal-level). Vagal is fast (~1 s), sympathetic slow (tens of seconds).
use crate::intero::{Band, Interocept, Sense, EV_INTERO};
use crate::person::Body;
use bodysim_core::component;
use bodysim_core::prelude::*;
use serde::{Deserialize, Serialize};

component! {
    pub struct Cardio = "cardio" { pub symp: Relax, pub vagal: Relax, pub racing_band: Band }
}

/// The DOWN channel from psyche: goal-level autonomic targets (0-1 each).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct AutonomicTarget { pub symp: f64, pub vagal: f64 }

pub const EV_AUTONOMIC: &str = "autonomic_target";

pub struct CardioPlugin;

impl Plugin for CardioPlugin {
    fn name(&self) -> &'static str { "cardio" }
    fn register(&self, r: &mut Registry) {
        r.component::<Cardio>();
        r.external_event(EV_AUTONOMIC);
        r.constants_ron("body.cardio", include_str!("../../../content/constants/body/cardio.ron"));
        r.system(SystemDescriptor {
            name: "autonomic",
            reads: &["cardio", "body"],
            writes: &["cardio"],
            handles: &["init", EV_AUTONOMIC, "activity_set", "sleep_onset", "wake"],
            schedules: &[EV_INTERO],
            consts: &["body.cardio.hr_intrinsic_bpm", "body.cardio.k_vagal_bpm", "body.cardio.k_symp_bpm", "body.cardio.tau_vagal_s",
                      "body.cardio.tau_symp_s", "body.cardio.vagal_rest", "body.cardio.symp_rest", "body.cardio.symp_per_met",
                      "body.cardio.racing_noticeable_bpm", "body.cardio.racing_pressing_bpm", "body.cardio.racing_urgent_bpm"],
            handler: autonomic,
            ..Default::default()
        });
        r.bounded(BoundedDecl {
            kind: "cardio", field: "symp", min: 0.0, max: 1.0,
            raised_by: &[Mechanism { system: "autonomic", on: EV_AUTONOMIC }],
            lowered_by: &[Mechanism { system: "autonomic", on: EV_AUTONOMIC }],
            tau_hint: Some(Span::mins(5)), rest: RestBound::Neither,
        });
    }
}

/// Heart rate now, bpm.
pub fn hr_bpm(w: &World, c: &Cardio, t: SimTime) -> f64 {
    w.k("body.cardio.hr_intrinsic_bpm") - w.k("body.cardio.k_vagal_bpm") * c.vagal.at(t) + w.k("body.cardio.k_symp_bpm") * c.symp.at(t)
}

fn autonomic(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    let people: Vec<EntityId> = if ev.is("init") { w.entities_with(w.kind_of::<Cardio>().unwrap()) } else { vec![ev.subject] };
    for e in people {
        let (Some(c), Some(b)) = (w.get::<Cardio>(e), w.get::<Body>(e)) else { continue };
        let mut c = c.clone();
        // targets: from psyche if given, else from what the body is doing
        let (ts, tv) = match ev.decode::<AutonomicTarget>().filter(|_| ev.is(EV_AUTONOMIC)) {
            Some(t) => (t.symp.clamp(0.0, 1.0), t.vagal.clamp(0.0, 1.0)),
            None => {
                let met = b.met();
                let symp = (w.k("body.cardio.symp_rest") + w.k("body.cardio.symp_per_met") * (met - 1.0).max(0.0)).clamp(0.0, 1.0);
                let vagal = (w.k("body.cardio.vagal_rest") * if met < 1.0 { 1.2 } else { 1.0 / met.max(1.0) }).clamp(0.0, 1.0);
                (symp, vagal)
            }
        };
        c.symp = c.symp.reanchor(now, ts, Span::from_secs_f64(w.k("body.cardio.tau_symp_s")));
        c.vagal = c.vagal.reanchor(now, tv, Span::from_secs_f64(w.k("body.cardio.tau_vagal_s")));
        // "heart racing" is a band on the HR this regime settles to (both anchors converge)
        let settled = w.k("body.cardio.hr_intrinsic_bpm") - w.k("body.cardio.k_vagal_bpm") * tv + w.k("body.cardio.k_symp_bpm") * ts;
        let band = Band::of(settled, w.k("body.cardio.racing_noticeable_bpm"), w.k("body.cardio.racing_pressing_bpm"), w.k("body.cardio.racing_urgent_bpm"));
        if band != c.racing_band {
            out.cancel_pending_why(e, EV_INTERO, "heart rate band settles");
            out.schedule_with(now + Span::from_secs_f64(w.k("body.cardio.tau_symp_s")), EV_INTERO, e, "heart rate band settles", &Interocept { sense: Sense::HeartRacing, band, rising: band.rank() > c.racing_band.rank() });
            c.racing_band = band;
        }
        out.set(e, c);
    }
    out
}
