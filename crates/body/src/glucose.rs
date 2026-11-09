//! Blood glucose and insulin: the Bergman minimal model linearised about basal - glucose relaxes
//! to basal at the glucose-effectiveness rate `Sg` (0.026-0.036 /min),
//! kicked by each absorbed-carbohydrate window; insulin follows glucose excess and decays with its
//! own clearance. The full minimal model has an explicit closed-form solution; this is the simpler
//! decoupled linear case, and it is marked as such.
use crate::metabolism::Absorbed;
use bodysim_core::component;
use bodysim_core::prelude::*;

component! {
    /// mg/dL and µU/mL, both anchors toward basal.
    pub struct Glucose = "glucose" { pub g: Relax, pub i: Relax }
}

pub struct GlucosePlugin;

impl Plugin for GlucosePlugin {
    fn name(&self) -> &'static str { "glucose" }
    fn register(&self, r: &mut Registry) {
        r.component::<Glucose>();
        r.constants_ron("body.glucose", include_str!("../../../content/constants/body/glucose.ron"));
        r.system(SystemDescriptor {
            name: "glycemia",
            reads: &["glucose"],
            writes: &["glucose"],
            handles: &["init", "absorbed"],
            consts: &["body.glucose.g_basal_mg_dl", "body.glucose.i_basal_uu_ml", "body.glucose.sg_per_min", "body.glucose.mg_dl_per_g_carb",
                      "body.glucose.insulin_tau_s", "body.glucose.insulin_gain_uu_per_mg_dl"],
            handler: glycemia,
            ..Default::default()
        });
        r.bounded(BoundedDecl {
            kind: "glucose", field: "g", min: 30.0, max: 400.0,
            raised_by: &[Mechanism { system: "glycemia", on: "absorbed" }],
            lowered_by: &[Mechanism { system: "glycemia", on: "absorbed" }],
            tau_hint: Some(Span::hours(2)), rest: RestBound::Neither,
        });
    }
}

fn glycemia(w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
    let mut out = Out::new();
    let now = ctx.now;
    let people: Vec<EntityId> = if ev.is("init") { w.entities_with(w.kind_of::<Glucose>().unwrap()) } else { vec![ev.subject] };
    let gb = w.k("body.glucose.g_basal_mg_dl");
    let ib = w.k("body.glucose.i_basal_uu_ml");
    let tau_g = Span::from_secs_f64(60.0 / w.k("body.glucose.sg_per_min"));
    let tau_i = Span::from_secs_f64(w.k("body.glucose.insulin_tau_s"));
    for e in people {
        let Some(gl) = w.get::<Glucose>(e) else { continue };
        let mut gl = gl.clone();
        let kick = if ev.is("absorbed") { ev.decode::<Absorbed>().map(|a| a.carb_g * w.k("body.glucose.mg_dl_per_g_carb")).unwrap_or(0.0) } else { 0.0 };
        let g_now = gl.g.at(now) + kick;
        gl.g = Relax::new(g_now, gb, now, tau_g);
        // insulin: excess glucose raises the target it relaxes toward
        let excess = (g_now - gb).max(0.0);
        gl.i = Relax::new(gl.i.at(now), ib + w.k("body.glucose.insulin_gain_uu_per_mg_dl") * excess, now, tau_i);
        out.set(e, gl);
    }
    out
}
