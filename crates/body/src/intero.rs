//! THE UP CHANNEL. The body emits interoceptive events, never continuous streams,
//! and only when a BAND boundary is crossed - the crossing time computed from the trajectory, not
//! polled. The mind reads these; it never reads the raw variable.
use bodysim_core::prelude::*;
use bodysim_core::value_enum;
use serde::{Deserialize, Serialize};

pub const EV_INTERO: &str = "intero";

value_enum! {
    /// What the body can report. One variant per Tier-1 signal that reaches awareness.
    pub enum Sense { Hunger, Thirst, Bladder, Bowel, Fatigue, Sleepy, Cold, Hot, Pain, Breathless, Nausea, HeartRacing, Shaky, Itch, Ache }
}

value_enum! {
    /// Five bands on the gate (the s99 translator law: bands, not labels, and never a raw number).
    pub enum Band { None, Trace, Noticeable, Pressing, Urgent }
}

impl Band {
    pub fn rank(self) -> u8 { Band::ALL.iter().position(|b| *b == self).unwrap_or(0) as u8 }
    pub fn at_least(self, other: Band) -> bool { self.rank() >= other.rank() }
    /// The band a value falls in, given the three ascending thresholds
    /// (Noticeable, Pressing, Urgent). Below the first is `Trace`; at or below zero, `None`.
    pub fn of(x: f64, noticeable: f64, pressing: f64, urgent: f64) -> Band {
        // a scheduled crossing lands on its threshold to the nanosecond, not to the bit:
        // count "within a millionth of the band width" as reached
        let eps = 1e-6 * (urgent - noticeable).abs().max(1e-9);
        if x + eps >= urgent { Band::Urgent }
        else if x + eps >= pressing { Band::Pressing }
        else if x + eps >= noticeable { Band::Noticeable }
        else if x > eps { Band::Trace }
        else { Band::None }
    }
    /// The value at which the NEXT band up begins, if there is one.
    pub fn next_threshold(self, noticeable: f64, pressing: f64, urgent: f64) -> Option<f64> {
        match self {
            Band::None | Band::Trace => Some(noticeable),
            Band::Noticeable => Some(pressing),
            Band::Pressing => Some(urgent),
            Band::Urgent => None,
        }
    }
    /// The value below which this band ends, if there is one.
    pub fn prev_threshold(self, noticeable: f64, pressing: f64, urgent: f64) -> Option<f64> {
        match self {
            Band::None => None,
            Band::Trace => Some(0.0),
            Band::Noticeable => Some(noticeable),
            Band::Pressing => Some(pressing),
            Band::Urgent => Some(urgent),
        }
    }
}

/// The payload of every `intero` event. Nothing else crosses the body→mind boundary.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Interocept { pub sense: Sense, pub band: Band, pub rising: bool }

/// Schedule the next band crossing of a trajectory, in whichever direction it is travelling.
/// A boundary this segment will never reach is simply not scheduled: a band edge is a QUESTION
/// ("will it cross?"), not a requirement - unlike a drain or a boil, where `Never` is a bug.
#[allow(clippy::too_many_arguments)]
pub fn schedule_band_change<T: Trajectory>(
    out: &mut Out, now: SimTime, traj: &T, band: Band, sense: Sense,
    noticeable: f64, pressing: f64, urgent: f64,
    subject: EntityId, why: &'static str,
) {
    let x = traj.at(now);
    let rising = traj.limit() > x;
    let target = if rising { band.next_threshold(noticeable, pressing, urgent) } else { band.prev_threshold(noticeable, pressing, urgent) };
    let Some(target) = target else { return };
    let reach = if rising { traj.reach_up(target) } else { traj.reach_down(target) };
    let band_then = if rising { Band::of(target, noticeable, pressing, urgent) } else {
        // just below the boundary we are falling through
        Band::of(target - 1e-3 * (urgent - noticeable).abs().max(1e-9), noticeable, pressing, urgent)
    };
    let payload = Interocept { sense, band: band_then, rising };
    match reach {
        Reach::At(t) => { out.schedule_with(t.max(now), EV_INTERO, subject, why, &payload); }
        Reach::Already => { out.schedule_with(now, EV_INTERO, subject, why, &payload); }
        Reach::Never { .. } => {}
    }
}
