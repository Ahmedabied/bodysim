//! The world: entities, one store per registered kind, edges, constants. READ-ONLY to systems.
//! There is NO `get_mut`. The only writer is `Engine::apply`.
use crate::component::{AnyComponent, AnyStore, Component, ComponentKind, Signature, SparseSet};
use crate::constants::Constants;
use crate::edges::{Edges, RelKind, RelTraits};
use crate::entities::Entities;
use crate::event::EventKind;
use crate::id::{EntityId, FieldId};
use crate::content::LoadedProto;
use crate::time::SimTime;
use crate::value::Value;
use indexmap::IndexMap;
use std::cell::{Cell, RefCell};

crate::component! {
    /// A human-readable name. Core-owned; used by content and the CLI.
    pub struct Label = "label" { pub name: String }
}

/// Name → id tables, fixed at `finalize`. Ids are indices in sorted-name order.
#[derive(Clone, Default)]
pub struct Kinds {
    pub components: IndexMap<&'static str, ComponentKind>,
    pub fields: Vec<&'static [&'static str]>,
    pub events: IndexMap<&'static str, EventKind>,
    pub relations: IndexMap<&'static str, RelKind>,
    pub rel_traits: Vec<RelTraits>,
    /// system name → id, so a code rule can name the one system allowed to do something
    pub systems: IndexMap<&'static str, crate::id::SystemId>,
}
impl Kinds {
    pub fn component(&self, name: &str) -> Option<ComponentKind> { self.components.get(name).copied() }
    pub fn component_name(&self, k: ComponentKind) -> &'static str { self.components.get_index(k.0 as usize).map(|(n, _)| *n).unwrap_or("?") }
    pub fn field(&self, k: ComponentKind, name: &str) -> Option<FieldId> {
        self.fields.get(k.0 as usize)?.iter().position(|f| *f == name).map(|i| FieldId(i as u16))
    }
    pub fn field_name(&self, k: ComponentKind, f: FieldId) -> &'static str {
        self.fields.get(k.0 as usize).and_then(|fs| fs.get(f.0 as usize)).copied().unwrap_or("?")
    }
    pub fn fields_of(&self, k: ComponentKind) -> &'static [&'static str] { self.fields.get(k.0 as usize).copied().unwrap_or(&[]) }
    pub fn event(&self, name: &str) -> Option<EventKind> { self.events.get(name).copied() }
    pub fn event_static(&self, name: &str) -> Option<&'static str> { self.events.get_key_value(name).map(|(k, _)| *k) }
    pub fn event_name(&self, k: EventKind) -> &'static str { self.events.get_index(k.0 as usize).map(|(n, _)| *n).unwrap_or("?") }
    pub fn relation(&self, name: &str) -> Option<RelKind> { self.relations.get(name).copied() }
    pub fn relation_static(&self, name: &str) -> Option<&'static str> { self.relations.get_key_value(name).map(|(k, _)| *k) }
    pub fn relation_name(&self, k: RelKind) -> &'static str { self.relations.get_index(k.0 as usize).map(|(n, _)| *n).unwrap_or("?") }
}

/// The payload of `EV_LAW`: one constant's new value (`plugin.name`), or a switch (`no_dots`) set to 1 or 0.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LawSet { pub name: String, pub value: f64 }

pub struct World {
    pub now: SimTime,
    /// THE LAWS IN FORCE: constants amended and switches flipped by `EV_LAW` externals. Read first by
    /// `k`/`k_opt`, and by `law` for the switches. Part of the state hash only once non-empty, so a
    /// world that never had a law hashes exactly as it did before laws existed (the replay law).
    pub laws: std::collections::BTreeMap<String, f64>,
    pub entities: Entities,
    pub kinds: Kinds,
    pub(crate) stores: Vec<Box<dyn AnyStore>>,
    pub(crate) signatures: Vec<Signature>,
    pub edges: Edges,
    pub consts: Constants,
    pub protos: IndexMap<String, LoadedProto>,
    pub(crate) names: IndexMap<String, EntityId>,
    pub(crate) current_system: Cell<Option<u16>>,
    pub(crate) system_reads: Vec<Signature>,
    pub(crate) system_consts: Vec<Vec<&'static str>>,
    /// the same lists as bits over constant rows, so the declared check is one bit test, not a list walk
    system_const_rows: Vec<Vec<u64>>,
    pub(crate) undeclared_reads: RefCell<IndexMap<(u16, u16), u64>>,
    pub(crate) undeclared_consts: RefCell<IndexMap<(u16, String), u64>>,
    consts_read: RefCell<ConstReads>,
    pub(crate) label_kind: ComponentKind,
}

impl World {
    pub(crate) fn new(kinds: Kinds, stores: Vec<Box<dyn AnyStore>>, consts: Constants, protos: IndexMap<String, LoadedProto>, system_reads: Vec<Signature>, system_consts: Vec<Vec<&'static str>>) -> World {
        let label_kind = kinds.component(Label::NAME).expect("label registered by core");
        let system_const_rows = system_consts.iter().map(|names| {
            let mut bits = vec![0u64; consts.rows.len().div_ceil(64)];
            for n in names { if let Some(i) = consts.rows.get_index_of(*n) { bits[i / 64] |= 1 << (i % 64); } }
            bits
        }).collect();
        World {
            now: SimTime::ZERO,
            laws: std::collections::BTreeMap::new(),
            entities: Entities::new(),
            edges: Edges::new(kinds.rel_traits.clone()),
            kinds,
            stores,
            signatures: Vec::new(),
            consts,
            protos,
            names: IndexMap::new(),
            current_system: Cell::new(None),
            system_reads,
            system_const_rows,
            system_consts,
            undeclared_reads: RefCell::new(IndexMap::new()),
            undeclared_consts: RefCell::new(IndexMap::new()),
            consts_read: RefCell::new(ConstReads::default()),
            label_kind,
        }
    }

    fn note_read(&self, k: ComponentKind) {
        if let Some(s) = self.current_system.get() {
            if !self.system_reads[s as usize].contains(k) {
                *self.undeclared_reads.borrow_mut().entry((s, k.0)).or_insert(0) += 1;
            }
        }
    }

    pub fn kind_of<T: Component>(&self) -> Option<ComponentKind> { self.kinds.component(T::NAME) }
    pub fn alive(&self, e: EntityId) -> bool { self.entities.alive(e) }
    pub fn store(&self, k: ComponentKind) -> &dyn AnyStore { &*self.stores[k.0 as usize] }

    pub fn get<T: Component>(&self, e: EntityId) -> Option<&T> {
        let k = self.kinds.component(T::NAME)?;
        self.note_read(k);
        self.stores[k.0 as usize].as_any().downcast_ref::<SparseSet<T>>()?.get(e)
    }
    pub fn get_any(&self, e: EntityId, k: ComponentKind) -> Option<&dyn AnyComponent> {
        self.note_read(k);
        self.stores.get(k.0 as usize)?.get_any(e)
    }
    pub fn has(&self, e: EntityId, k: ComponentKind) -> bool {
        self.note_read(k);
        self.stores.get(k.0 as usize).is_some_and(|s| s.has(e))
    }
    pub fn has_named(&self, e: EntityId, kind: &str) -> bool { self.kinds.component(kind).is_some_and(|k| self.has(e, k)) }
    pub fn field(&self, e: EntityId, k: ComponentKind, f: FieldId) -> Option<Value> { self.get_any(e, k)?.field(f) }
    pub fn field_named(&self, e: EntityId, kind: &str, field: &str) -> Option<Value> {
        let k = self.kinds.component(kind)?;
        let f = self.kinds.field(k, field)?;
        self.field(e, k, f)
    }
    /// All (entity, component) of a kind. Order is deterministic (store order), not canonical.
    pub fn query<T: Component>(&self) -> Box<dyn Iterator<Item = (EntityId, &T)> + '_> {
        match self.kinds.component(T::NAME) {
            Some(k) => {
                self.note_read(k);
                match self.stores[k.0 as usize].as_any().downcast_ref::<SparseSet<T>>() {
                    Some(s) => Box::new(s.iter()),
                    None => Box::new(std::iter::empty()),
                }
            }
            None => Box::new(std::iter::empty()),
        }
    }
    /// Entities of a kind in canonical (index) order.
    pub fn entities_with(&self, k: ComponentKind) -> Vec<EntityId> {
        self.note_read(k);
        self.stores.get(k.0 as usize).map(|s| s.entities()).unwrap_or_default()
    }
    pub fn signature(&self, e: EntityId) -> Signature {
        if !self.alive(e) { return Signature::default(); }
        self.signatures.get(e.index as usize).copied().unwrap_or_default()
    }
    pub fn resource<T: Component>(&self) -> Option<&T> { self.get::<T>(EntityId::WORLD) }
    pub fn label(&self, e: EntityId) -> Option<&str> {
        self.stores[self.label_kind.0 as usize].as_any().downcast_ref::<SparseSet<Label>>()?.get(e).map(|l| l.name.as_str())
    }
    pub fn find(&self, name: &str) -> Option<EntityId> { self.names.get(name).copied().filter(|e| self.alive(*e)) }
    pub fn entities_alive(&self) -> Vec<EntityId> { self.entities.iter_alive() }
    pub fn changed_at(&self, e: EntityId, k: ComponentKind) -> Option<SimTime> { self.stores.get(k.0 as usize)?.changed_at(e) }

    /// A constant by full name (`plugin.name`). Missing ⇒ panic: `finalize` verified every declared
    /// constant; an undeclared read is counted as a defect and still answered if the row exists.
    ///
    /// One hash lookup finds the row; the declared check and the read count then go by its index.
    pub fn k(&self, name: &str) -> f64 {
        let row = self.consts.rows.get_full(name);
        if let Some(s) = self.current_system.get() {
            let declared = match row {
                Some((i, _, _)) if self.system_const_rows[s as usize].get(i / 64).is_some_and(|w| w & (1 << (i % 64)) != 0) => true,
                // a row added after the world was built has no bit yet: ask the list
                _ => self.system_consts[s as usize].contains(&name),
            };
            if !declared { *self.undeclared_consts.borrow_mut().entry((s, name.to_string())).or_insert(0) += 1; }
        }
        self.consts_read.borrow_mut().count(row.map(|(i, _, _)| i), name);
        if let Some(v) = self.laws.get(name) { return *v; }
        match row {
            Some((_, _, r)) => r.value,
            None => panic!("constant {name} is not in content (declare it in a constants RON with unit, source and status)"),
        }
    }
    pub fn k_opt(&self, name: &str) -> Option<f64> { self.laws.get(name).copied().or_else(|| self.consts.value(name)) }
    /// A named behaviour switch (no dots: not a constant), off until a law sets it above one half.
    /// Old behaviour on the left of the `if`, new on the right; old logs never take the right branch.
    pub fn law(&self, name: &str) -> bool { self.laws.get(name).is_some_and(|v| *v > 0.5) }

    /// Clone a component and set one field on the copy (for data-driven effects).
    pub fn with_field_set(&self, e: EntityId, k: ComponentKind, f: FieldId, v: Value) -> Result<Box<dyn AnyComponent>, String> {
        let c = self.get_any(e, k).ok_or_else(|| format!("{e} has no {}", self.kinds.component_name(k)))?;
        let mut b = c.clone_box();
        b.set_field(f, v).map_err(|err| format!("{}.{}: {err}", self.kinds.component_name(k), self.kinds.field_name(k, f)))?;
        Ok(b)
    }
    /// Instantiate a proto's components (content) as boxed components, ready for `Out::spawn`.
    pub fn instantiate_proto(&self, name: &str) -> Result<Vec<Box<dyn AnyComponent>>, String> {
        let p = self.protos.get(name).ok_or_else(|| format!("unknown proto {name}"))?;
        Ok(p.components.clone())
    }

    // ---- engine-internal mutation (pub(crate)) ----
    pub(crate) fn set_signature(&mut self, e: EntityId, s: Signature) {
        let i = e.index as usize;
        if i >= self.signatures.len() { self.signatures.resize(i + 1, Signature::default()); }
        self.signatures[i] = s;
    }
    pub(crate) fn store_mut(&mut self, k: ComponentKind) -> &mut dyn AnyStore { &mut *self.stores[k.0 as usize] }
    // THE NAME TABLE: a name finds the live entity holding that label with the LOWEST id. One rule for spawn,
    // despawn, rename and restore, so `find` answers the same while running and after an epoch (adoption looks
    // scene entities up through it). Spawn order is not part of the state; the lowest id is.
    pub(crate) fn note_label(&mut self, e: EntityId, c: &dyn AnyComponent) {
        if let Some(l) = c.as_any().downcast_ref::<Label>() { self.note_name(l.name.clone(), e); }
    }
    fn note_name(&mut self, name: String, e: EntityId) {
        match self.names.get(&name) {
            Some(held) if *held < e && self.entities.alive(*held) => {}
            _ => { self.names.insert(name, e); }
        }
    }
    /// `e` no longer holds the labels it held (despawned, label removed or replaced): each name passes to
    /// its next holder, if there is one.
    pub(crate) fn forget_labels_of(&mut self, e: EntityId) {
        let held: Vec<String> = self.names.iter().filter(|(_, v)| **v == e).map(|(k, _)| k.clone()).collect();
        for name in held {
            self.names.shift_remove(&name);
            let next = self.stores[self.label_kind.0 as usize].as_any().downcast_ref::<SparseSet<Label>>()
                .and_then(|st| st.iter().filter(|(x, l)| *x != e && l.name == name && self.entities.alive(*x)).map(|(x, _)| x).min());
            if let Some(x) = next { self.names.insert(name, x); }
        }
    }
    /// After `e`'s label was replaced: the old name passes on, the new one is noted.
    pub(crate) fn relabel(&mut self, e: EntityId) {
        self.forget_labels_of(e);
        if let Some(name) = self.label(e).map(str::to_string) { self.note_name(name, e); }
    }
    /// Signatures and the name table from what the stores hold (THE EPOCH, after a restore).
    pub(crate) fn rebuild_indexes(&mut self) {
        self.signatures.clear();
        self.names.clear();
        for e in self.entities.iter_alive() {
            let s = Signature::of_stores(self, e);
            self.set_signature(e, s);
        }
        let labels: Vec<(EntityId, String)> = self.stores[self.label_kind.0 as usize].as_any().downcast_ref::<SparseSet<Label>>()
            .map(|st| { let mut v: Vec<(EntityId, String)> = st.iter().map(|(e, l)| (e, l.name.clone())).collect(); v.sort(); v }).unwrap_or_default();
        // ascending ids, first one kept: the lowest live id per name, as `note_name` keeps it
        for (e, name) in labels { if self.entities.alive(e) { self.names.entry(name).or_insert(e); } }
    }
    pub fn undeclared_reads(&self) -> Vec<((u16, u16), u64)> { self.undeclared_reads.borrow().iter().map(|(k, v)| (*k, *v)).collect() }
    pub fn undeclared_consts(&self) -> Vec<((u16, String), u64)> { self.undeclared_consts.borrow().iter().map(|(k, v)| (k.clone(), *v)).collect() }
    /// Reads per constant, in first-read order (names with no row, read just before their panic or set only by a
    /// law, come last).
    pub fn consts_read(&self) -> Vec<(String, u64)> {
        let r = self.consts_read.borrow();
        r.first_read.iter().map(|i| (self.consts.rows.get_index(*i as usize).map(|(k, _)| k.clone()).unwrap_or_default(), r.by_row[*i as usize]))
            .chain(r.other.iter().map(|(k, v)| (k.clone(), *v))).collect()
    }
    pub fn stores_len(&self) -> usize { self.stores.len() }
}

/// How often each constant was read: by row index, so counting a read allocates nothing and hashes nothing.
#[derive(Default)]
struct ConstReads { by_row: Vec<u64>, first_read: Vec<u32>, other: IndexMap<String, u64> }
impl ConstReads {
    fn count(&mut self, row: Option<usize>, name: &str) {
        match row {
            Some(i) => {
                if i >= self.by_row.len() { self.by_row.resize(i + 1, 0); }
                if self.by_row[i] == 0 { self.first_read.push(i as u32); }
                self.by_row[i] += 1;
            }
            None => match self.other.get_mut(name) { Some(n) => *n += 1, None => { self.other.insert(name.to_string(), 1); } },
        }
    }
}
