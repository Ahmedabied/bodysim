//! Sleep: the two-process model, Process S as a `Relax` between transitions (τ_wake 18.2 h toward
//! 1, τ_sleep 4.2 h toward 0), sleep onset/offset as SCHEDULED crossings of the C-modulated
//! thresholds ("a single cheap root-find you can schedule and forget").
//! Sound above the stage's arousal threshold wakes her. Caffeine raises the onset threshold.
use crate::circadian::process_c;
use crate::intero::{schedule_band_change, Band, Interocept, Sense, EV_INTERO};
use crate::pk::caffeine_occupancy;
use bodysim_core::relax::first_crossing_scan;
use bodysim_core::{component, value_enum};
use bodysim_core::prelude::*;

value_enum! {
    pub enum SleepState { Awake, Asleep }
}

component! {
    /// Process S and the current state. `since` anchors the stage estimate while asleep.
    pub struct Sleep = "sleep" { pub s: Relax, pub state: SleepState, pub since: SimTime, pub sleepy_band: Band }
}

pub const EV_ONSET: &str = "sleep_onset";
pub const EV_WAKE: &str = "wake";
pub const EV_CHECK: &str = "sleep_check";
pub const EV_RESUME: &str = "resume_sleep";
pub const WHY_NOCTURIA: &str = "woken by a full bladder";
pub const WHY_ONSET: &str = "sleep pressure meets the onset threshold";
pub const WHY_WAKE: &str = "sleep pressure falls to the offset threshold";
pub const WHY_SLEEPY: &str = "sleepiness band crossing";

pub struct SleepPlugin;

impl Plugin for SleepPlugin {
    fn name(&self) -> &'static str { "sleep" }
    fn register(&self, r: &mut Registry) {
        r.component::<Sleep>();
        r.event(EV_ONSET);
        // Waking a sleeper can also come from outside the simulation. The same kind, now injectable: no new rule, no new constant, so it is outside the replay's
        // content hash, and an old log holds no such injection and replays exactly as it always did.
        r.external_event(EV_WAKE);
        r.external_event(EV_CHECK);
        r.external_event(EV_RESUME);
        r.constants_ron("body.sleep", include_str!("../../../content/constants/body/sleep.ron"));
        r.system(SystemDescriptor {
            name: "sleeper",
            reads: &["sleep", "circadian", "body", "position", "caffeine"],
            reads_opt: &["room_noise", "clock"],
            writes: &["sleep"],
            handles: &["init", EV_ONSET, EV_WAKE, EV_CHECK, EV_INTERO, EV_RESUME],
            handles_opt: &["sound"],
            schedules: &[EV_ONSET, EV_WAKE, EV_CHECK, EV_INTERO],
            consts: &["body.sleep.tau_wake_h", "body.sleep.tau_sleep_h", "body.sleep.mu_wake", "body.sleep.mu_sleep",
                      "body.sleep.h_on", "body.sleep.h_off", "body.sleep.c_amplitude", "body.sleep.caffeine_threshold_raise", "body.sleep.wake_grace_s",
                      "body.sleep.sleepy_noticeable", "body.sleep.sleepy_pressing", "body.sleep.arousal_db_deep",
                      "body.sleep.arousal_db_light", "body.sleep.deep_first_h", "body.pk.caffeine_ec50_mg_l", "body.sleep.resume_margin"],
            handler: sleeper,
            ..Default::default()
        });
        r.bounded(BoundedDecl {
            kind: "sleep", field: "s", min: 0.0, max: 1.0,
            raised_by: &[Mechanism { system: "sleeper", on: EV_WAKE }],
            lowered_by: &[Mechanism { system: "sleeper", on: EV_ONSET }],
            tau_hint: Some(Span::hours(18)), rest: RestBound::Neither,
        });
    }
}

/// The C-modulated onset threshold at `t` (raised by caffeine).
fn onset_threshold(w: &World, e: EntityId, t: SimTime) -> f64 {
    w.k("body.sleep.h_on") + w.k("body.sleep.c_amplitude") * process_c(w, e, t) + w.k("body.sleep.caffeine_threshold_raise") * caffeine_occupancy(w, e, t)
}
fn offset_threshold(w: &World, e: EntityId, t: SimTime) -> f64 {
    w.k("body.sleep.h_off") + w.k("body.sleep.c_amplitude") * process_c(w, e, t)
}

/// Find when a transition becomes due, including pressure already beyond its threshold.
/// An interrupted sleeper gets time to resolve the interruption before automatic onset.
fn due_offset(f: impl Fn(f64) -> f64, horizon: f64, grace: f64) -> Option<f64> {
    if f(0.0) >= 0.0 {
        if f(grace) >= 0.0 { Some(grace) }
        else { first_crossing_scan(|s| f(s + grace), horizon - grace, 288).map(|s| s + grace) }
    } else {
        first_crossing_scan(f, horizon, 288)
    }
}

/// Schedule the next due transition, with a bounded search for future crossings.
fn schedule_transition(w: &World, e: EntityId, sl: &Sleep, now: SimTime, out: &mut Out) {
    out.cancel_pending(e, EV_ONSET);
    out.cancel_pending(e, EV_WAKE);
    let horizon = Span::hours(36);
    match sl.state {
        SleepState::Awake => {
            let f = |s: f64| { let t = now + Span::from_secs_f64(s); sl.s.at(t) - onset_threshold(w, e, t) };
            let grace = (w.k("body.sleep.wake_grace_s") - (now - sl.since).as_secs_f64()).max(0.0);
            if let Some(s) = due_offset(f, horizon.as_secs_f64(), grace) {
                out.schedule_at(now + Span::from_secs_f64(s).max(Span::secs(1)), EV_ONSET, e, WHY_ONSET);
            }
        }
        SleepState::Asleep => {
            let f = |s: f64| { let t = now + Span::from_secs_f64(s); offset_threshold(w, e, t) - sl.s.at(t) };
            if let Some(s) = due_offset(f, horizon.as_secs_f64(), 0.0) {
                out.schedule_at(now + Span::from_secs_f64(s).max(Span::secs(1)), EV_WAKE, e, WHY_WAKE);
            }
        }
    }
}

fn sleeper(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    if ev.is("init") {
        for e in w.entities_with(w.kind_of::<Sleep>().unwrap()) { out.schedule_at(now, EV_CHECK, e, "sleep schedule at start"); }
        return out;
    }
    // sound: does it wake anyone asleep in that room?
    if ev.is("sound") {
        for (e, sl) in w.query::<Sleep>() {
            if sl.state != SleepState::Asleep { continue; }
            let Some(Value::Id(room)) = w.field_named(e, "position", "room") else { continue };
            let Some(Value::F64(spl)) = w.field_named(room, "room_noise", "spl_db") else { continue };
            let asleep_h = (now - sl.since).as_hours_f64();
            let thr = if asleep_h < w.k("body.sleep.deep_first_h") { w.k("body.sleep.arousal_db_deep") } else { w.k("body.sleep.arousal_db_light") };
            if spl >= thr { out.cancel_pending(e, EV_WAKE); out.schedule_at(now, EV_WAKE, e, "woken by noise"); }
        }
        return out;
    }
    let e = ev.subject;
    let Some(sl) = w.get::<Sleep>(e) else { return out };
    // an urgent bladder or bowel wakes a sleeper (nocturia); other interoception is ignored here
    if ev.is(EV_INTERO) {
        if sl.state == SleepState::Asleep {
            if let Some(i) = ev.decode::<Interocept>() {
                if matches!(i.sense, Sense::Bladder | Sense::Bowel) && i.band == Band::Urgent {
                    out.cancel_pending(e, EV_WAKE);
                    out.schedule_at(now, EV_WAKE, e, WHY_NOCTURIA);
                }
            }
        }
        return out;
    }
    let mut sl = sl.clone();
    match ev.kind_name {
        "sleep_onset" if sl.state == SleepState::Awake => {
            sl.s = sl.s.reanchor(now, w.k("body.sleep.mu_sleep"), Span::from_hours_f64(w.k("body.sleep.tau_sleep_h")));
            sl.state = SleepState::Asleep;
            sl.since = now;
        }
        "wake" if sl.state == SleepState::Asleep => {
            sl.s = sl.s.reanchor(now, w.k("body.sleep.mu_wake"), Span::from_hours_f64(w.k("body.sleep.tau_wake_h")));
            sl.state = SleepState::Awake;
            sl.since = now;
        }
        // back to bed after a night interruption: only if sleep pressure is still well above the
        // offset threshold (else she is simply up for the day)
        "resume_sleep" if sl.state == SleepState::Awake => {
            if sl.s.at(now) < offset_threshold(w, e, now) + w.k("body.sleep.resume_margin") { return out; }
            sl.s = sl.s.reanchor(now, w.k("body.sleep.mu_sleep"), Span::from_hours_f64(w.k("body.sleep.tau_sleep_h")));
            sl.state = SleepState::Asleep;
            sl.since = now;
        }
        "sleep_check" => {
            // (re)anchor S for the current state: the first time, from content
            let (mu, tau) = match sl.state {
                SleepState::Awake => (w.k("body.sleep.mu_wake"), w.k("body.sleep.tau_wake_h")),
                SleepState::Asleep => (w.k("body.sleep.mu_sleep"), w.k("body.sleep.tau_sleep_h")),
            };
            sl.s = sl.s.reanchor(now, mu, Span::from_hours_f64(tau));
        }
        _ => return out,
    }
    schedule_transition(w, e, &sl, now, &mut out);
    // sleepiness bands on S (awake only)
    let (n, p) = (w.k("body.sleep.sleepy_noticeable"), w.k("body.sleep.sleepy_pressing"));
    let u = onset_threshold(w, e, now);
    let band = if sl.state == SleepState::Asleep { Band::None } else { Band::of(sl.s.at(now), n, p, u) };
    if band != sl.sleepy_band {
        out.schedule_with(now, EV_INTERO, e, WHY_SLEEPY, &Interocept { sense: Sense::Sleepy, band, rising: band.rank() > sl.sleepy_band.rank() });
        sl.sleepy_band = band;
    }
    out.cancel_pending_why(e, EV_INTERO, WHY_SLEEPY);
    if sl.state == SleepState::Awake { schedule_band_change(&mut out, now, &sl.s, sl.sleepy_band, Sense::Sleepy, n, p, u, e, WHY_SLEEPY); }
    // A six-hour re-check also recovers overdue transitions after state adoption.
    out.cancel_pending(e, EV_CHECK);
    out.schedule_in(now, Span::hours(6), EV_CHECK, e, "sleep schedule re-check");
    out.set(e, sl);
    out
}

#[cfg(test)]
mod tests {
    use super::due_offset;
    const HORIZON: f64 = 36.0 * 3600.0;

    #[test]
    fn overdue_pressure_recovers_without_another_crossing() {
        assert_eq!(due_offset(|_| 0.2, HORIZON, 0.0), Some(0.0));
        assert_eq!(due_offset(|_| 0.2, HORIZON, 600.0), Some(600.0));
        assert_eq!(due_offset(|_| -0.2, HORIZON, 0.0), None);
        assert!((due_offset(|s| s - 7200.0, HORIZON, 0.0).unwrap() - 7200.0).abs() < 1e-6);
    }

    #[test]
    fn interruption_can_move_the_threshold_before_returning_to_sleep() {
        let f = |s: f64| (s - 100.0) * (s - 7200.0);
        assert!((due_offset(f, HORIZON, 600.0).unwrap() - 7200.0).abs() < 1e-6);
    }
}
