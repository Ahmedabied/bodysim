//! The log is the truth; state is a fold; snapshots are an optimisation (-27).
//! Frame: `u32 len | postcard(Envelope) | u32 crc32`. Written at the single application point.
use crate::id::{EdgeId, EntityId, EventId};
use crate::proposal::Violation;
use crate::time::SimTime;
use crate::value::Value;
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;

pub const SCHEMA_VERSION: u16 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ChangeRecord {
    Set { kind: String, fields: Vec<(String, Value, Value)> },
    Insert { kind: String, fields: Vec<(String, Value)> },
    Remove { kind: String },
    EdgeOpen { id: EdgeId, kind: String, dst: EntityId, closed: Vec<EdgeId> },
    EdgeClose { id: EdgeId, superseded_by: Option<EdgeId> },
    Spawn { kinds: Vec<String> },
    Despawn { closed_edges: usize },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Record {
    Header { seed: u64, schema_version: u16, checkpoint_every: u64, created: String, content_hash: [u8; 32], plugins: Vec<String> },
    Event { id: EventId, at: SimTime, kind: String, subject: EntityId, cause: Option<EventId>, why: String },
    Applied { event: EventId, by: String, entity: EntityId, change: ChangeRecord },
    Rejected { event: EventId, by: String, violation: Violation },
    /// The ONLY door for the outside world. Logged before it is scheduled, so replay reproduces it.
    External { id: EventId, after_events: u64, now: SimTime, at: SimTime, kind: String, subject: EntityId, payload: Vec<u8> },
    /// A crossing was requested beyond the asymptote: refused, never scheduled.
    Unreachable { event: EventId, system: String, why: String, threshold: f64, x_inf: f64 },
    Note { event: EventId, system: String, text: String },
    Defect { event: Option<EventId>, text: String },
    Dropped { id: EventId, at: SimTime, kind: String, subject: EntityId, reason: String },
    Checkpoint { at: SimTime, events: u64, hash: [u8; 32] },
    Invariant { name: String, entity: EntityId, threshold: f64, deadline: SimTime, value_at_deadline: f64, ok: bool },
    /// THE FRESH EPOCH: the whole state at `at`, written by `bodysim epoch` at index 1 of a fresh log. The fold
    /// starts here; `prior` is the archived log this one continues. Appended LAST in the enum so every older record
    /// keeps its postcard discriminant.
    Epoch { at: SimTime, events: u64, prior: String, prior_hash: [u8; 32], prior_records: u64, state: Vec<u8> },
    /// Typed event data for consumers such as rendering and observation receipts. Appended to
    /// preserve the postcard discriminants of every existing log record.
    EventPayload { id: EventId, data: Vec<u8> },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Envelope { pub schema_version: u16, pub rule_versions: u32, pub record: Record }

pub struct LogWriter {
    file: Option<BufWriter<File>>,
    pub memory: Vec<Envelope>,
    keep_memory: bool,
    pub rule_versions: u32,
    pub count: u64,
}

impl LogWriter {
    /// Keep every record in memory (tests, short runs, `why` queries).
    pub fn memory() -> LogWriter { LogWriter { file: None, memory: Vec::new(), keep_memory: true, rule_versions: 0, count: 0 } }
    /// Keep nothing (replay verification, benchmarks).
    pub fn discard() -> LogWriter { LogWriter { file: None, memory: Vec::new(), keep_memory: false, rule_versions: 0, count: 0 } }
    pub fn file(path: impl AsRef<Path>, append: bool) -> io::Result<LogWriter> {
        let f = if append { OpenOptions::new().create(true).append(true).open(path)? } else { File::create(path)? };
        Ok(LogWriter { file: Some(BufWriter::new(f)), memory: Vec::new(), keep_memory: false, rule_versions: 0, count: 0 })
    }
    pub fn with_memory(mut self, keep: bool) -> LogWriter { self.keep_memory = keep; self }
    pub fn append(&mut self, record: Record) {
        let env = Envelope { schema_version: SCHEMA_VERSION, rule_versions: self.rule_versions, record };
        if let Some(f) = &mut self.file {
            let bytes = postcard::to_stdvec(&env).expect("postcard");
            let crc = crc32fast::hash(&bytes);
            let _ = f.write_all(&(bytes.len() as u32).to_le_bytes());
            let _ = f.write_all(&bytes);
            let _ = f.write_all(&crc.to_le_bytes());
        }
        if self.keep_memory { self.memory.push(env); }
        self.count += 1;
    }
    pub fn flush(&mut self) { if let Some(f) = &mut self.file { let _ = f.flush(); } }
    pub fn records(&self) -> impl Iterator<Item = &Record> { self.memory.iter().map(|e| &e.record) }
}

impl Drop for LogWriter {
    fn drop(&mut self) { self.flush(); }
}

pub fn read_log(path: impl AsRef<Path>) -> io::Result<Vec<Envelope>> {
    let mut bytes = Vec::new();
    File::open(path)?.read_to_end(&mut bytes)?;
    parse_log(&bytes)
}

pub fn parse_log(bytes: &[u8]) -> io::Result<Vec<Envelope>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        let len = u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        i += 4;
        if i + len + 4 > bytes.len() {
            return Err(io::Error::new(io::ErrorKind::UnexpectedEof, format!("truncated frame at record {}", out.len())));
        }
        let body = &bytes[i..i + len];
        let crc = u32::from_le_bytes(bytes[i + len..i + len + 4].try_into().unwrap());
        if crc32fast::hash(body) != crc {
            return Err(io::Error::new(io::ErrorKind::InvalidData, format!("crc mismatch at record {}", out.len())));
        }
        let env: Envelope = postcard::from_bytes(body).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
        out.push(env);
        i += len + 4;
    }
    Ok(out)
}
