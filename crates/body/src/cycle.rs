//! Age and the menstrual cycle: a phase state machine on a per-person cycle length (mean 27.7 ± 3.4
//! d, luteal 12.4 d,) with the luteal BBT shift (+0.3 to +0.7 °C) read by
//! thermoregulation. Phases are SCHEDULED at their boundaries; the day is derived from time.
use bodysim_core::{component, value_enum};
use bodysim_core::prelude::*;

value_enum! {
    pub enum Phase { Menstrual, Follicular, Ovulatory, Luteal }
}

component! {
    /// `t0` = the first day of the current cycle; `length_d`/`luteal_d` per person.
    pub struct Cycle = "cycle" { pub t0: SimTime, pub length_d: f64, pub luteal_d: f64, pub phase: Phase }
}

pub const EV_PHASE: &str = "cycle_phase";

pub struct CyclePlugin;

impl Plugin for CyclePlugin {
    fn name(&self) -> &'static str { "cycle" }
    fn register(&self, r: &mut Registry) {
        r.component::<Cycle>();
        r.event(EV_PHASE);
        r.constants_ron("body.cycle", include_str!("../../../content/constants/body/cycle.ron"));
        r.system(SystemDescriptor {
            name: "cycling",
            reads: &["cycle"],
            writes: &["cycle"],
            handles: &["init", EV_PHASE],
            schedules: &[EV_PHASE],
            consts: &["body.cycle.menses_d", "body.cycle.ovulatory_window_d", "body.cycle.bbt_shift_c"],
            handler: cycling,
            ..Default::default()
        });
        r.rule(DeclRule::Range { kind: "cycle".into(), field: "length_d".into(), min: 20.0, max: 45.0 });
    }
}

pub fn day(c: &Cycle, t: SimTime) -> f64 { (t - c.t0).as_days_f64().rem_euclid(c.length_d) }
pub fn phase_at(w: &World, c: &Cycle, t: SimTime) -> Phase {
    let d = day(c, t);
    let ovul_start = c.length_d - c.luteal_d - w.k("body.cycle.ovulatory_window_d");
    if d < w.k("body.cycle.menses_d") { Phase::Menstrual } else if d < ovul_start { Phase::Follicular } else if d < c.length_d - c.luteal_d { Phase::Ovulatory } else { Phase::Luteal }
}
/// Core-temperature offset from the cycle (luteal rise).
pub fn bbt_shift_c(w: &World, c: &Cycle, t: SimTime) -> f64 { if phase_at(w, c, t) == Phase::Luteal { w.k("body.cycle.bbt_shift_c") } else { 0.0 } }
/// Next phase boundary after `t`.
fn next_boundary(w: &World, c: &Cycle, t: SimTime) -> SimTime {
    let d = day(c, t);
    let bounds = [w.k("body.cycle.menses_d"), c.length_d - c.luteal_d - w.k("body.cycle.ovulatory_window_d"), c.length_d - c.luteal_d, c.length_d];
    let next = bounds.iter().copied().find(|b| *b > d + 1e-6).unwrap_or(c.length_d);
    t + Span::from_days_f64(next - d)
}

fn cycling(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    let people: Vec<EntityId> = if ev.is("init") { w.entities_with(w.kind_of::<Cycle>().unwrap()) } else { vec![ev.subject] };
    for e in people {
        let Some(c) = w.get::<Cycle>(e) else { continue };
        let mut c = c.clone();
        if day(&c, now) >= c.length_d - 1e-9 || (now - c.t0).as_days_f64() >= c.length_d { c.t0 = now - Span::from_days_f64(day(&c, now)); }
        c.phase = phase_at(w, &c, now);
        out.cancel_pending(e, EV_PHASE);
        out.schedule_at(next_boundary(w, &c, now), EV_PHASE, e, "cycle phase boundary");
        out.set(e, c);
    }
    out
}
