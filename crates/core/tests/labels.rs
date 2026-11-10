//! `World::find(name)`: the live entity with that label and the lowest id, the same answer while running
//! and after an epoch restore, through spawns, despawns and renames. Epoch adoption looks scene entities
//! up through `find`, so an answer that changes across a restore attaches content to the wrong entity.
use bodysim_core::prelude::*;
use bodysim_core::registry::Blueprint;
use bodysim_core::world::Label;

mod naming {
    use super::*;
    pub struct Naming;
    impl Plugin for Naming {
        fn name(&self) -> &'static str { "naming" }
        fn register(&self, r: &mut Registry) {
            r.external_event("make");
            r.external_event("drop");
            r.external_event("rename");
            r.system(SystemDescriptor {
                name: "namer", reads: &["label"], writes: &["label"], spawns: &["label"], may_despawn: true,
                handles: &["make", "drop", "rename"], handler: namer, ..Default::default()
            });
        }
    }
    fn namer(_w: &World, ev: &Event, ctx: &mut Ctx) -> Out {
        let mut out = Out::new();
        match ev.kind_name {
            "make" => { let e = ctx.reserve(); out.spawn(e, vec![Box::new(Label { name: ev.decode::<String>().unwrap() })]); }
            "drop" => { out.despawn(ev.subject); }
            "rename" => { out.set(ev.subject, Label { name: ev.decode::<String>().unwrap() }); }
            _ => {}
        }
        out
    }
}

struct Run { bp: Blueprint, e: Engine, t: SimTime }
impl Run {
    fn new() -> Run {
        let mut r = Registry::new();
        r.add(&naming::Naming);
        let bp = r.finalize().unwrap_or_else(|d| panic!("{d:?}"));
        let e = bp.build(1, LogWriter::memory(), 1000).unwrap();
        Run { bp, e, t: SimTime::ZERO }
    }
    fn act(&mut self, kind: &str, subject: EntityId, name: &str) {
        self.t += Span::mins(1);
        self.e.inject(kind, subject, Payload::of(&name.to_string()), Some(self.t)).unwrap();
        self.e.run_until(self.t);
    }
    fn make(&mut self, name: &str) -> EntityId {
        let before = self.e.world.entities_alive();
        self.act("make", EntityId::WORLD, name);
        self.e.world.entities_alive().into_iter().find(|x| !before.contains(x)).expect("spawned")
    }
    /// What `find` answers after the state is written as an epoch and read back.
    fn find_after_restore(&self, name: &str) -> Option<EntityId> {
        let st = self.e.epoch_state().unwrap();
        let (back, _) = Engine::from_epoch(&self.bp, 1, LogWriter::memory(), 1000, &st).unwrap();
        back.world.find(name)
    }
}

#[test]
fn two_of_one_name_find_the_lowest_id_live_and_after_a_restore() {
    let mut r = Run::new();
    let p = r.make("p");
    let a = r.make("x");
    r.act("drop", p, "");
    let b = r.make("x"); // takes p's freed index, lower than a's
    assert!(b < a, "the free list should hand back p's index: {b} vs {a}");
    assert_eq!(r.e.world.find("x"), Some(b), "live");
    assert_eq!(r.find_after_restore("x"), Some(b), "after a restore");
    // the lower one goes: the other one still answers to the name
    r.act("drop", b, "");
    assert_eq!(r.e.world.find("x"), Some(a), "live, after the lower one is gone");
    assert_eq!(r.find_after_restore("x"), Some(a));
}

#[test]
fn dropping_the_newer_of_two_keeps_the_name_for_the_older() {
    let mut r = Run::new();
    let a = r.make("x");
    let b = r.make("x");
    r.act("drop", b, "");
    assert_eq!(r.e.world.find("x"), Some(a), "a still holds the label");
    assert_eq!(r.find_after_restore("x"), Some(a));
}

#[test]
fn a_rename_moves_the_name() {
    let mut r = Run::new();
    let a = r.make("old");
    r.act("rename", a, "new");
    assert_eq!(r.e.world.find("new"), Some(a), "live: the new name");
    assert_eq!(r.e.world.find("old"), None, "live: the old name is gone");
    assert_eq!((r.find_after_restore("new"), r.find_after_restore("old")), (Some(a), None));
    // a rename onto a name a lower id already holds does not take it
    let c = r.make("c");
    r.act("rename", c, "new");
    assert_eq!(r.e.world.find("new"), Some(a.min(c)));
    assert_eq!(r.find_after_restore("new"), Some(a.min(c)));
}
