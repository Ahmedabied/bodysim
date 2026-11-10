//! Reading a constant counts the read, and a read the system did not declare is counted as a defect.
use bodysim_core::prelude::*;

mod reader {
    use super::*;
    pub struct Reader;
    impl Plugin for Reader {
        fn name(&self) -> &'static str { "reader" }
        fn register(&self, r: &mut Registry) {
            r.constants_ron("t", r#"[
  (name: "a", value: 1.5, unit: "1", range: None, varies_with: "-", source: "test", status: Tunable, note: None),
  (name: "b", value: 2.5, unit: "1", range: None, varies_with: "-", source: "test", status: Tunable, note: None),
  (name: "c", value: 3.5, unit: "1", range: None, varies_with: "-", source: "test", status: Tunable, note: None),
]"#);
            r.system(SystemDescriptor { name: "reader", handles: &["init"], consts: &["t.c", "t.a"], handler: read, ..Default::default() });
        }
    }
    fn read(w: &World, _: &Event, _: &mut Ctx) -> Out {
        // c declared and read once; b NOT declared, read once; a declared, read twice
        assert_eq!(w.k("t.c") + w.k("t.b") + w.k("t.a") + w.k("t.a"), 3.5 + 2.5 + 1.5 + 1.5);
        Out::new()
    }
}

#[test]
fn reads_are_counted_in_first_read_order_and_the_undeclared_one_is_named() {
    let mut r = Registry::new();
    r.add(&reader::Reader);
    let bp = r.finalize().unwrap_or_else(|d| panic!("{d:?}"));
    let mut e = bp.build(1, LogWriter::memory(), 1000).unwrap();
    e.run_until(SimTime::ZERO + Span::mins(1));
    let reads: Vec<(String, u64)> = e.world.consts_read();
    assert_eq!(reads, vec![("t.c".to_string(), 1), ("t.b".to_string(), 1), ("t.a".to_string(), 2)]);
    let undeclared: Vec<(String, u64)> = e.world.undeclared_consts().into_iter().map(|((_, n), c)| (n, c)).collect();
    assert_eq!(undeclared, vec![("t.b".to_string(), 1)]);
}
