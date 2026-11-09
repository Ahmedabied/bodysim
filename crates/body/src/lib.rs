//! bodysim: human physiology as closed-form models, advanced from one threshold crossing to the next.
//!
//! One plugin per subsystem, one owner per Tier-1 variable. Every quantity is a closed form
//! evaluated when looked at; the body wakes only when a band boundary is crossed, and the crossing
//! time is computed analytically. The body speaks upward only through `intero` - typed band
//! crossings, never a raw variable.
pub mod person;
pub mod room;
pub mod intero;
pub mod hydration;
pub mod thermo;
pub mod circadian;
pub mod sleep;
pub mod metabolism;
pub mod digestion;
pub mod glucose;
pub mod cardio;
pub mod cortisol;
pub mod cycle;
pub mod pk;

use bodysim_core::id::EntityId;
use bodysim_core::registry::Registry;
use serde::{Deserialize, Serialize};

/// The wire shape of the world's `drink` event payload (the world's drink container).
/// Body and world never depend on each other; an event's payload layout is the contract between
/// them (postcard is positional: same fields, same order, same types).
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct DrinkPayload { pub by: EntityId, pub volume_l: f64 }

/// The Tier-1 subsystems the vertical slice needs.
pub fn register_slice(r: &mut Registry) {
    r.add(&room::RoomPlugin);
    r.add(&person::PersonPlugin);
    r.add(&hydration::HydrationPlugin);
    r.add(&thermo::ThermoPlugin);
}

/// Every Tier-1 body plugin, in build order.
pub fn register_body(r: &mut Registry) {
    r.add(&room::RoomPlugin);
    r.add(&person::PersonPlugin);
    r.add(&circadian::CircadianPlugin);
    r.add(&pk::PkPlugin);
    r.add(&sleep::SleepPlugin);
    r.add(&thermo::ThermoPlugin);
    r.add(&metabolism::MetabolismPlugin);
    r.add(&digestion::DigestionPlugin);
    r.add(&glucose::GlucosePlugin);
    r.add(&hydration::HydrationPlugin);
    r.add(&cardio::CardioPlugin);
    r.add(&cortisol::CortisolPlugin);
    r.add(&cycle::CyclePlugin);
}

/// Three example adults, as content.
pub fn register_people(r: &mut Registry) {
    r.scene_ron(include_str!("../../../content/people/adult-a.ron"));
    r.scene_ron(include_str!("../../../content/people/adult-b.ron"));
    r.scene_ron(include_str!("../../../content/people/adult-c.ron"));
}
