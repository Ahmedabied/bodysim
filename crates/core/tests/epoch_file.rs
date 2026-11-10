//! An epoch log is a fresh file. Written over an existing one, it would destroy the history it was meant to
//! follow (the prior log is archived, not replaced), so an existing file is refused and left as it was.
use bodysim_core::epoch::{epoch_of, write_epoch_log};
use bodysim_core::log::read_log;
use bodysim_core::prelude::*;

fn fresh_engine() -> (bodysim_core::registry::Blueprint, Engine) {
    let bp = Registry::new().finalize().unwrap_or_else(|d| panic!("{d:?}"));
    let mut e = bp.build(1, LogWriter::memory(), 1000).unwrap();
    e.run_until(SimTime::ZERO + Span::mins(1));
    (bp, e)
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("bodysim-epoch-file-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn an_existing_file_is_refused_and_left_as_it_was() {
    let (bp, e) = fresh_engine();
    let st = e.epoch_state().unwrap();
    let path = scratch("prior.log");
    std::fs::write(&path, b"the prior history").unwrap();
    let r = write_epoch_log(&path, &bp, 1, 1000, "now", &st, "prior.log", [0; 32], 0, &[]);
    assert!(r.is_err(), "an epoch was written over an existing file");
    assert!(r.unwrap_err().contains("already exists"));
    assert_eq!(std::fs::read(&path).unwrap(), b"the prior history");
    std::fs::remove_file(&path).unwrap();
}

#[test]
fn a_fresh_path_gets_the_epoch() {
    let (bp, e) = fresh_engine();
    let st = e.epoch_state().unwrap();
    let path = scratch("fresh.log");
    let _ = std::fs::remove_file(&path);
    write_epoch_log(&path, &bp, 1, 1000, "now", &st, "prior.log", [0; 32], 0, &[]).unwrap();
    let back = epoch_of(&read_log(&path).unwrap()).unwrap().expect("an epoch record");
    assert_eq!((back.now, back.hash), (st.now, st.hash));
    std::fs::remove_file(&path).unwrap();
}
