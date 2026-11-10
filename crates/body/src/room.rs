//! The smallest world a body can live in: one room with steady air, where the body is, and drinks,
//! meals and toilet visits arriving from outside. The body reads air temperature through
//! `position` -> `room_air`; a richer world (weather, heating, bedding, several rooms) can replace
//! this plugin without touching the body.
//!
//! The room is held at 27 C: there is no bedding or wardrobe in this world, and 27 C is close to
//! thermoneutral for a lightly clothed adult, so core temperature stays in its normal daily range.
//!
//! The room also has a toilet: a pressing bladder or bowel is answered a few minutes later. Without
//! it nobody would ever empty their bowel, the colon would pass its bound after about two days, and
//! every digestion write after that would be refused. Nobody goes in their sleep: asleep, a pressing
//! urge waits; an urgent one wakes the sleeper (the sleep plugin's nocturia), and what presses on
//! waking is answered then. After a nocturia wake she tries to go back to bed; the sleep plugin's
//! resume rule decides whether she does.
use crate::digestion::{Digestion, EV_DEFECATE};
use crate::hydration::{Bladder, EV_VOID};
use crate::sleep::{EV_RESUME, WHY_NOCTURIA};
use crate::intero::{Band, Interocept, Sense, EV_INTERO};
use bodysim_core::prelude::*;
use bodysim_core::relax::Relax;
use bodysim_core::{component, value_enum};

value_enum! {
    /// Where in the room the body is.
    pub enum Spot { Bed, Seat, Standing }
}

component! {
    /// Which room a body is in, and where.
    pub struct Position = "position" { pub room: EntityId, pub spot: Spot }
}

component! {
    /// A room's air temperature, as a trajectory (constant here).
    pub struct RoomAir = "room_air" { pub t: Relax }
}

pub struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn name(&self) -> &'static str { "room" }
    fn register(&self, r: &mut Registry) {
        r.component::<Position>();
        r.component::<RoomAir>();
        r.external_event("drink");
        r.system(SystemDescriptor {
            name: "toilet",
            reads: &["bladder", "digestion"],
            reads_opt: &["sleep"],
            handles: &[EV_INTERO],
            handles_opt: &["wake"],
            schedules: &[EV_VOID, EV_DEFECATE],
            schedules_opt: &[EV_RESUME],
            handler: toilet,
            ..Default::default()
        });
        r.constants_ron("world.thermal", r#"[
  (name: "outdoor_c", value: 12.0, unit: "degC", range: None, varies_with: "weather",
   source: "illustrative mild day; read only when a body is in no room", status: Tunable, note: None),
]"#);
        r.scene_ron(r#"(entities: [
  (name: "room", components: { "room_air": (t: (x0: 27.0, x_inf: 27.0, t0: 0, tau: 3600000000000)) }),
])"#);
    }
}

/// Minutes between a pressing urge and the toilet visit.
const TOILET_DELAY_MIN: i64 = 5;

/// Minutes from a night visit back to bed.
const BACK_TO_BED_MIN: i64 = 10;

fn toilet(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let person = ev.subject;
    let pressing = |b: Band| b.rank() >= Band::Pressing.rank();
    if ev.is("wake") {
        // what pressed while she slept is answered now that she is up
        if w.get::<Bladder>(person).is_some_and(|b| pressing(b.band)) {
            out.schedule_in(ctx.now, Span::mins(TOILET_DELAY_MIN), EV_VOID, person, "a pressing urge on waking, answered at the toilet");
        }
        if w.get::<Digestion>(person).is_some_and(|d| pressing(d.bowel_band)) {
            out.schedule_in(ctx.now, Span::mins(TOILET_DELAY_MIN), EV_DEFECATE, person, "a pressing urge on waking, answered at the toilet");
        }
        if ev.why == WHY_NOCTURIA {
            out.schedule_in(ctx.now, Span::mins(TOILET_DELAY_MIN + BACK_TO_BED_MIN), EV_RESUME, person, "back to bed after a night visit");
        }
        return out;
    }
    let Some(i) = ev.decode::<Interocept>() else { return out };
    if !i.rising || !pressing(i.band) { return out; }
    let kind = match i.sense { Sense::Bladder => EV_VOID, Sense::Bowel => EV_DEFECATE, _ => return out };
    // asleep, the urge waits: an urgent one wakes her, and the wake above answers it
    if matches!(w.field_named(person, "sleep", "state"), Some(Value::Enum(1))) { return out; }
    out.schedule_in(ctx.now, Span::mins(TOILET_DELAY_MIN), kind, person, "a pressing urge, answered at the toilet");
    out
}
