//! An epoch restore must rebuild each store in the order `World::query` walks it, not sorted: a system that
//! schedules one event per entity numbers them in that order, and event ids key the RNG streams. The state hash
//! is canonical, so it cannot see the difference; this test can.
use bodysim_core::component::{AnyStore, SparseSet};
use bodysim_core::id::EntityId;
use bodysim_core::time::SimTime;
use bodysim_core::world::Label;

fn order(s: &SparseSet<Label>) -> Vec<EntityId> { s.iter().map(|(e, _)| e).collect() }
fn hash(s: &SparseSet<Label>) -> [u8; 32] { let mut h = blake3::Hasher::new(); s.hash_into(&mut h); *h.finalize().as_bytes() }

#[test]
fn a_restored_store_walks_its_rows_in_the_order_it_had() {
    let mut s = SparseSet::<Label>::new();
    for i in 0..4 { s.insert(EntityId::new(i, 0), Label { name: format!("p{i}") }, SimTime::ZERO); }
    s.remove(EntityId::new(0, 0)); // the last row moves into the hole: the store order is no longer sorted
    let before = order(&s);
    let mut sorted = before.clone();
    sorted.sort();
    assert_ne!(before, sorted, "the test needs a store whose order is not sorted");

    let mut from_ron = SparseSet::<Label>::new();
    from_ron.restore_ron(&s.snapshot_ron().unwrap()).unwrap();
    assert_eq!(order(&from_ron), before);
    let mut from_bytes = SparseSet::<Label>::new();
    from_bytes.restore(&s.snapshot()).unwrap();
    assert_eq!(order(&from_bytes), before);
    assert_eq!(hash(&from_ron), hash(&s)); // the hash never depended on it
}

#[test]
fn a_state_written_sorted_by_an_older_binary_still_restores() {
    let rows = vec![(EntityId::new(1, 0), SimTime::ZERO, Label { name: "a".into() }), (EntityId::new(3, 0), SimTime::ZERO, Label { name: "b".into() })];
    let mut s = SparseSet::<Label>::new();
    s.restore_ron(&bodysim_core::ron::to_string(&rows).unwrap()).unwrap();
    assert_eq!(order(&s), vec![EntityId::new(1, 0), EntityId::new(3, 0)]);
}
