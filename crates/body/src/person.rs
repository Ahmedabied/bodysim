//! What every body needs before any subsystem can compute anything: size, age, sex, and what it
//! is doing right now. Content-written, read by every body plugin.
use bodysim_core::prelude::*;
use bodysim_core::{component, value_enum};

value_enum! {
    pub enum Sex { Female, Male }
}

value_enum! {
    /// What the body is doing - sets metabolic rate and effort. (A coarse scale; the full
    /// Compendium of Physical Activities MET table is not modelled.)
    pub enum Activity { Sleeping, Resting, Light, Moderate, Vigorous }
}

component! {
    /// A body's fixed dimensions. `bsa_m2` is DuBois: 0.202*W^0.425*H^0.725 (W kg, H m).
    pub struct Body = "body" {
        pub mass_kg: f64, pub height_cm: f64, pub age_yr: f64, pub sex: Sex,
        pub bsa_m2: f64, pub clo: f64, pub activity: Activity,
    }
}

pub const EV_ACTIVITY: &str = "activity_set";

pub struct PersonPlugin;

impl Plugin for PersonPlugin {
    fn name(&self) -> &'static str { "person" }
    fn register(&self, r: &mut Registry) {
        r.component::<Body>();
        r.external_event(EV_ACTIVITY);
        r.system(SystemDescriptor {
            name: "person",
            reads: &["body"],
            writes: &["body.activity"],
            handles: &[EV_ACTIVITY],
            handles_opt: &["sleep_onset", "wake"],
            handler: person,
            ..Default::default()
        });
        r.rule(DeclRule::Range { kind: "body".into(), field: "mass_kg".into(), min: 2.0, max: 300.0 });
        r.rule(DeclRule::Range { kind: "body".into(), field: "bsa_m2".into(), min: 0.15, max: 3.0 });
        r.rule(DeclRule::Range { kind: "body".into(), field: "clo".into(), min: 0.0, max: 4.0 });
        r.rule(DeclRule::Requires { kind: "body".into(), other: "position".into() });
    }
}

fn person(w: &World, ev: &Event, _ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let Some(b) = w.get::<Body>(ev.subject) else { return out };
    let activity = match ev.kind_name {
        "activity_set" => match ev.decode::<Activity>() { Some(a) => a, None => return out },
        "sleep_onset" => Activity::Sleeping,
        "wake" => Activity::Resting,
        _ => return out,
    };
    if activity != b.activity { out.set(ev.subject, Body { activity, ..b.clone() }); }
    out
}

/// DuBois body surface area (pythermalcomfort `utilities.py`).
pub fn dubois_bsa(mass_kg: f64, height_cm: f64) -> f64 {
    0.202 * mass_kg.powf(0.425) * (height_cm / 100.0).powf(0.725)
}

impl Body {
    /// Metabolic multiplier over resting (1 met), by activity.
    pub fn met(&self) -> f64 {
        match self.activity {
            Activity::Sleeping => 0.7,
            Activity::Resting => 1.0,
            Activity::Light => 1.4,
            Activity::Moderate => 3.0,
            Activity::Vigorous => 6.0,
        }
    }
}
