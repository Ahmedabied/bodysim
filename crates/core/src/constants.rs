//! Every number lives in content with unit, source and status. No literal constants in code.
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ConstStatus {
    Verified,
    Unverified,
    Contested(String),
    /// no sourced value exists; the row says so and the report counts it
    Tunable,
    /// the replication watchlist says: do not build a mechanism on this. The registry refuses a reader.
    DoNotBuildOn,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ConstRow {
    pub name: String,
    pub value: f64,
    pub unit: String,
    #[serde(default)]
    pub range: Option<(f64, f64)>,
    #[serde(default)]
    pub varies_with: Option<String>,
    pub source: String,
    pub status: ConstStatus,
    #[serde(default)]
    pub note: Option<String>,
}

/// The constants table: a world resource, loaded from `content/constants/**.ron`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Constants { pub rows: IndexMap<String, ConstRow> }

impl Constants {
    pub fn get(&self, name: &str) -> Option<&ConstRow> { self.rows.get(name) }
    pub fn value(&self, name: &str) -> Option<f64> { self.rows.get(name).map(|r| r.value) }
    pub fn insert(&mut self, prefix: &str, row: ConstRow) -> Result<(), String> {
        let key = if prefix.is_empty() { row.name.clone() } else { format!("{prefix}.{}", row.name) };
        if self.rows.contains_key(&key) { return Err(format!("duplicate constant {key}")); }
        self.rows.insert(key, row);
        Ok(())
    }
    pub fn load_ron(&mut self, prefix: &str, text: &str) -> Result<usize, String> {
        let rows: Vec<ConstRow> = crate::registry::from_ron(text).map_err(|e| format!("constants {prefix}: {e}"))?;
        let n = rows.len();
        for r in rows { self.insert(prefix, r)?; }
        Ok(n)
    }
    pub fn tunables(&self) -> Vec<&str> { self.rows.iter().filter(|(_, r)| r.status == ConstStatus::Tunable).map(|(k, _)| k.as_str()).collect() }
    pub fn contested(&self) -> Vec<&str> { self.rows.iter().filter(|(_, r)| matches!(r.status, ConstStatus::Contested(_))).map(|(k, _)| k.as_str()).collect() }
    pub fn hash_into(&self, h: &mut blake3::Hasher) {
        for (k, r) in &self.rows { h.update(k.as_bytes()); h.update(&r.value.to_le_bytes()); }
    }
}
