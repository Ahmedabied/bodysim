//! A domain-free plugin that exercises every primitive: a `gauge` relaxes toward a keyed-random
//! target on each `pulse`, is lowered on `drain` at an analytic crossing, a `tally` counts drains.
//! Used by core's tests and `bodysim --plugins fixture`.
use crate::component;
use crate::edges::RelTraits;
use crate::engine::Ctx;
use crate::event::Event;
use crate::id::EntityId;
use crate::proposal::Out;
use crate::registry::{BoundedDecl, Mechanism, Plugin, Registry, RestBound, SystemDescriptor};
use crate::relax::{Relax, Trajectory};
use crate::rng::normal;
use crate::time::Span;
use crate::world::World;

component! {
    /// A relaxing number with a pulse counter.
    pub struct Gauge = "gauge" { pub x: Relax, pub pulses: i64 }
}
component! {
    /// Counts drains.
    pub struct Tally = "tally" { pub drains: i64 }
}

pub const CONSTANTS: &str = r#"[
  (name: "pulse_period_s", value: 600.0, unit: "s", source: "fixture", status: Tunable),
  (name: "tau_s", value: 1800.0, unit: "s", source: "fixture", status: Tunable),
  (name: "drain_at", value: 0.9, unit: "1", source: "fixture", status: Tunable),
  (name: "target_sd", value: 0.03, unit: "1", source: "fixture", status: Tunable),
]"#;

pub const PROTOS: &str = r#"[
  (name: "gauge_thing", components: { "gauge": (x: (x0: 0.0, x_inf: 0.0, t0: 0, tau: 1800000000000), pulses: 0), "tally": (drains: 0) }),
]"#;

pub const SCENE: &str = r#"(entities: [
  (name: "g1", proto: "gauge_thing"),
  (name: "g2", proto: "gauge_thing", components: { "gauge": (x: (x0: 0.5, x_inf: 0.5, t0: 0, tau: 1800000000000), pulses: 0) }),
  (name: "g3", proto: "gauge_thing", edges: [(kind: "near", dst: "g1")]),
])"#;

pub const RULES: &str = r#"[
  Range(kind: "tally", field: "drains", min: 0.0, max: 1000000000.0),
  Monotone(kind: "tally", field: "drains", dir: Up),
  Requires(kind: "tally", other: "gauge"),
]"#;

/// `unreachable`: ask for a crossing beyond the asymptote (must be refused and logged).
#[derive(Default)]
pub struct FixturePlugin { pub unreachable: bool }

impl Plugin for FixturePlugin {
    fn name(&self) -> &'static str { "fixture" }
    fn register(&self, r: &mut Registry) {
        r.component::<Gauge>();
        r.component::<Tally>();
        r.event("pulse");
        r.event("drain");
        r.external_event("poke");
        r.external_event("break");
        r.relation("near", RelTraits { symmetric: true, ..Default::default() });
        r.constants_ron("fixture", CONSTANTS);
        r.protos_ron(PROTOS);
        r.scene_ron(SCENE);
        r.rules_ron(RULES);
        r.system(SystemDescriptor {
            name: "pulser",
            reads: &["gauge"],
            writes: &["gauge"],
            handles: &["init", "pulse", "drain", "poke", "break"],
            schedules: &["pulse", "drain"],
            consts: &["fixture.pulse_period_s", "fixture.tau_s", "fixture.drain_at", "fixture.target_sd"],
            handler: if self.unreachable { pulser_unreachable } else { pulser },
            ..Default::default()
        });
        r.system(SystemDescriptor {
            name: "drainer",
            reads: &["tally"],
            writes: &["tally"],
            handles: &["drain"],
            handler: drainer,
            ..Default::default()
        });
        r.bounded(BoundedDecl {
            kind: "gauge", field: "x", min: 0.0, max: 1.2,
            raised_by: &[Mechanism { system: "pulser", on: "pulse" }],
            lowered_by: &[Mechanism { system: "pulser", on: "drain" }],
            tau_hint: Some(Span::secs(1800)), rest: RestBound::Neither,
        });
    }
}

fn pulse_common(w: &World, ev: &Event, ctx: &mut Ctx, threshold: f64) -> Out {
    let mut out = Out::new();
    let period = Span::from_secs_f64(w.k("fixture.pulse_period_s"));
    let tau = Span::from_secs_f64(w.k("fixture.tau_s"));
    let sd = w.k("fixture.target_sd");
    match ev.kind_name {
        "init" => {
            for e in w.entities_with(w.kind_of::<Gauge>().unwrap()) {
                out.schedule_in(ctx.now, period, "pulse", e, "fixture pulse period");
            }
        }
        "pulse" => {
            let e = ev.subject;
            let Some(g) = w.get::<Gauge>(e) else { return out };
            let target = 1.0 + sd * normal(ctx.rng(e));
            let x = g.x.reanchor(ctx.now, target, tau);
            out.cancel_pending(e, "drain");
            out.schedule_reach(ctx.now, x.reach_up(threshold), threshold, "drain", e, "gauge reaches drain_at");
            out.set(e, Gauge { x, pulses: g.pulses + 1 });
            out.schedule_in(ctx.now, period, "pulse", e, "fixture pulse period");
        }
        "drain" => {
            let e = ev.subject;
            let Some(g) = w.get::<Gauge>(e) else { return out };
            out.set(e, Gauge { x: g.x.reanchor(ctx.now, 0.0, tau), pulses: g.pulses });
        }
        "poke" => {
            let e = ev.subject;
            let delta: f64 = ev.decode().unwrap_or(0.1);
            if let Some(g) = w.get::<Gauge>(e) { out.set(e, Gauge { x: g.x.kick(ctx.now, delta), pulses: g.pulses }); }
        }
        "break" => {
            let e = ev.subject;
            if let Some(g) = w.get::<Gauge>(e) { out.set(e, Gauge { x: Relax::constant(5.0, ctx.now), pulses: g.pulses }); }
        }
        _ => {}
    }
    out
}

fn pulser(w: &World, ev: &Event, ctx: &mut Ctx) -> Out { let t = w.k("fixture.drain_at"); pulse_common(w, ev, ctx, t) }
fn pulser_unreachable(w: &World, ev: &Event, ctx: &mut Ctx) -> Out { pulse_common(w, ev, ctx, 2.0) }

fn drainer(w: &World, ev: &Event, _ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    if let Some(t) = w.get::<Tally>(ev.subject) { out.set(ev.subject, Tally { drains: t.drains + 1 }); }
    out
}

/// The registry every core test starts from.
pub fn registry(unreachable: bool) -> Registry {
    let mut r = Registry::new();
    r.add(&FixturePlugin { unreachable });
    r
}

#[allow(dead_code)]
fn _entity_id_used(_: EntityId) {}
