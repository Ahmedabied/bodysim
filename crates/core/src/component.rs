//! Components: registered by NAME, stored in one sparse set per kind, reflected field-by-field
//! so the engine can diff a proposed replacement, dispatch rules on touched fields, and
//! name `field/old/attempted` in every violation.
use crate::id::{EntityId, FieldId};
use crate::time::SimTime;
use crate::value::{Value, ValueError};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::any::Any;
use std::fmt::Debug;

#[derive(Copy, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, Debug, Default)]
pub struct ComponentKind(pub u16);

pub trait Component: Serialize + DeserializeOwned + Clone + PartialEq + Debug + Send + Sync + 'static {
    const NAME: &'static str;
    const FIELDS: &'static [&'static str];
    fn field(&self, i: FieldId) -> Option<Value>;
    fn set_field(&mut self, i: FieldId, v: Value) -> Result<(), ValueError>;
    fn field_id(name: &str) -> Option<FieldId> {
        Self::FIELDS.iter().position(|f| *f == name).map(|i| FieldId(i as u16))
    }
}

/// Type-erased component: what the engine, the log and the rules see.
pub trait AnyComponent: Any + Debug + Send + Sync {
    fn kind_name(&self) -> &'static str;
    fn field_names(&self) -> &'static [&'static str];
    fn field(&self, i: FieldId) -> Option<Value>;
    fn set_field(&mut self, i: FieldId, v: Value) -> Result<(), ValueError>;
    fn clone_box(&self) -> Box<dyn AnyComponent>;
    fn as_any(&self) -> &dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
    fn to_bytes(&self) -> Vec<u8>;
    fn eq_any(&self, other: &dyn AnyComponent) -> bool;
}

impl<T: Component> AnyComponent for T {
    fn kind_name(&self) -> &'static str { T::NAME }
    fn field_names(&self) -> &'static [&'static str] { T::FIELDS }
    fn field(&self, i: FieldId) -> Option<Value> { Component::field(self, i) }
    fn set_field(&mut self, i: FieldId, v: Value) -> Result<(), ValueError> { Component::set_field(self, i, v) }
    fn clone_box(&self) -> Box<dyn AnyComponent> { Box::new(self.clone()) }
    fn as_any(&self) -> &dyn Any { self }
    fn into_any(self: Box<Self>) -> Box<dyn Any> { self }
    fn to_bytes(&self) -> Vec<u8> { postcard::to_stdvec(self).expect("postcard") }
    fn eq_any(&self, other: &dyn AnyComponent) -> bool { other.as_any().downcast_ref::<T>().is_some_and(|o| o == self) }
}

impl Clone for Box<dyn AnyComponent> {
    fn clone(&self) -> Self { self.clone_box() }
}

pub fn downcast<T: Component>(c: &dyn AnyComponent) -> Option<&T> { c.as_any().downcast_ref::<T>() }

/// A bitset over registered component kinds (≤ 256).
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Signature(pub [u64; 4]);
impl Signature {
    pub const MAX_KINDS: usize = 256;
    pub fn from_kinds(kinds: impl IntoIterator<Item = ComponentKind>) -> Signature {
        let mut s = Signature::default();
        for k in kinds { s.insert(k); }
        s
    }
    pub fn insert(&mut self, k: ComponentKind) { self.0[(k.0 / 64) as usize] |= 1u64 << (k.0 % 64); }
    pub fn remove(&mut self, k: ComponentKind) { self.0[(k.0 / 64) as usize] &= !(1u64 << (k.0 % 64)); }
    pub fn contains(&self, k: ComponentKind) -> bool { self.0[(k.0 / 64) as usize] & (1u64 << (k.0 % 64)) != 0 }
    pub fn is_subset(&self, of: &Signature) -> bool { (0..4).all(|i| self.0[i] & !of.0[i] == 0) }
    pub fn is_empty(&self) -> bool { self.0.iter().all(|w| *w == 0) }
    pub fn iter(&self) -> impl Iterator<Item = ComponentKind> + '_ {
        (0..256u16).filter(move |k| self.contains(ComponentKind(*k))).map(ComponentKind)
    }
    pub fn with(mut self, k: ComponentKind) -> Signature { self.insert(k); self }
    pub fn without(mut self, k: ComponentKind) -> Signature { self.remove(k); self }
}
impl Debug for Signature {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter().map(|k| k.0)).finish()
    }
}

/// One sparse set per registered kind.
pub struct SparseSet<T: Component> {
    dense: Vec<T>,
    entities: Vec<EntityId>,
    sparse: Vec<u32>,
    changed_at: Vec<SimTime>,
}

impl<T: Component> Default for SparseSet<T> {
    fn default() -> Self { Self::new() }
}

impl<T: Component> SparseSet<T> {
    pub fn new() -> Self { SparseSet { dense: Vec::new(), entities: Vec::new(), sparse: Vec::new(), changed_at: Vec::new() } }
    fn slot(&self, e: EntityId) -> Option<usize> {
        let i = e.index as usize;
        if i < self.sparse.len() {
            let s = self.sparse[i];
            if s != u32::MAX && self.entities[s as usize] == e { return Some(s as usize); }
        }
        None
    }
    pub fn get(&self, e: EntityId) -> Option<&T> { self.slot(e).map(|s| &self.dense[s]) }
    pub fn insert(&mut self, e: EntityId, v: T, now: SimTime) -> Option<T> {
        if let Some(s) = self.slot(e) {
            self.changed_at[s] = now;
            return Some(std::mem::replace(&mut self.dense[s], v));
        }
        let i = e.index as usize;
        if i >= self.sparse.len() { self.sparse.resize(i + 1, u32::MAX); }
        self.sparse[i] = self.dense.len() as u32;
        self.dense.push(v);
        self.entities.push(e);
        self.changed_at.push(now);
        None
    }
    pub fn remove(&mut self, e: EntityId) -> Option<T> {
        let s = self.slot(e)?;
        let last = self.dense.len() - 1;
        self.sparse[e.index as usize] = u32::MAX;
        let v = self.dense.swap_remove(s);
        self.entities.swap_remove(s);
        self.changed_at.swap_remove(s);
        if s != last {
            let moved = self.entities[s];
            self.sparse[moved.index as usize] = s as u32;
        }
        Some(v)
    }
    pub fn iter(&self) -> impl Iterator<Item = (EntityId, &T)> { self.entities.iter().copied().zip(self.dense.iter()) }
    pub fn len(&self) -> usize { self.dense.len() }
    pub fn is_empty(&self) -> bool { self.dense.is_empty() }
}

/// Type-erased store.
pub trait AnyStore: Send + Sync {
    fn kind_name(&self) -> &'static str;
    fn field_names(&self) -> &'static [&'static str];
    fn has(&self, e: EntityId) -> bool;
    fn get_any(&self, e: EntityId) -> Option<&dyn AnyComponent>;
    fn insert_any(&mut self, e: EntityId, c: Box<dyn AnyComponent>, now: SimTime) -> Result<Option<Box<dyn AnyComponent>>, String>;
    fn remove_any(&mut self, e: EntityId) -> Option<Box<dyn AnyComponent>>;
    /// Typed deserialization of this kind from any serde deserializer (content loading).
    fn boxed_from_erased(&self, d: &mut dyn erased_serde::Deserializer) -> Result<Box<dyn AnyComponent>, String>;
    /// Entities holding this kind, in canonical (index) order.
    fn entities(&self) -> Vec<EntityId>;
    fn len(&self) -> usize;
    fn is_empty(&self) -> bool { self.len() == 0 }
    fn changed_at(&self, e: EntityId) -> Option<SimTime>;
    fn hash_into(&self, h: &mut blake3::Hasher);
    fn snapshot(&self) -> Vec<u8>;
    fn restore(&mut self, bytes: &[u8]) -> Result<(), String>;
    /// THE EPOCH's shape: self-describing rows `(entity, changed_at, component)`, so a component may grow a
    /// `#[serde(default)]` field between epochs and an old state still reads.
    fn snapshot_ron(&self) -> Result<String, String>;
    fn restore_ron(&mut self, text: &str) -> Result<(), String>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Component> AnyStore for SparseSet<T> {
    fn kind_name(&self) -> &'static str { T::NAME }
    fn field_names(&self) -> &'static [&'static str] { T::FIELDS }
    fn has(&self, e: EntityId) -> bool { self.slot(e).is_some() }
    fn get_any(&self, e: EntityId) -> Option<&dyn AnyComponent> { self.get(e).map(|c| c as &dyn AnyComponent) }
    fn insert_any(&mut self, e: EntityId, c: Box<dyn AnyComponent>, now: SimTime) -> Result<Option<Box<dyn AnyComponent>>, String> {
        let name = c.kind_name();
        let v = c.into_any().downcast::<T>().map_err(|_| format!("store {} given a {}", T::NAME, name))?;
        Ok(self.insert(e, *v, now).map(|old| Box::new(old) as Box<dyn AnyComponent>))
    }
    fn remove_any(&mut self, e: EntityId) -> Option<Box<dyn AnyComponent>> { self.remove(e).map(|c| Box::new(c) as Box<dyn AnyComponent>) }
    fn boxed_from_erased(&self, d: &mut dyn erased_serde::Deserializer) -> Result<Box<dyn AnyComponent>, String> {
        let c: T = erased_serde::deserialize(d).map_err(|e| format!("{}: {e}", T::NAME))?;
        Ok(Box::new(c))
    }
    fn entities(&self) -> Vec<EntityId> {
        let mut v = self.entities.clone();
        v.sort();
        v
    }
    fn len(&self) -> usize { self.dense.len() }
    fn changed_at(&self, e: EntityId) -> Option<SimTime> { self.slot(e).map(|s| self.changed_at[s]) }
    fn hash_into(&self, h: &mut blake3::Hasher) {
        h.update(T::NAME.as_bytes());
        h.update(&(self.dense.len() as u64).to_le_bytes());
        for e in self.entities() {
            let c = self.get(e).expect("canonical iteration");
            h.update(&e.index.to_le_bytes());
            h.update(&e.gen.to_le_bytes());
            h.update(&c.to_bytes());
        }
    }
    fn snapshot(&self) -> Vec<u8> {
        let rows: Vec<(EntityId, SimTime, &T)> = self.entities().into_iter().map(|e| (e, self.changed_at(e).unwrap_or_default(), self.get(e).unwrap())).collect();
        postcard::to_stdvec(&rows).expect("postcard")
    }
    fn restore(&mut self, bytes: &[u8]) -> Result<(), String> {
        let rows: Vec<(EntityId, SimTime, T)> = postcard::from_bytes(bytes).map_err(|e| e.to_string())?;
        *self = SparseSet::new();
        for (e, t, c) in rows { self.insert(e, c, t); }
        Ok(())
    }
    fn snapshot_ron(&self) -> Result<String, String> {
        let rows: Vec<(EntityId, SimTime, &T)> = self.entities().into_iter().map(|e| (e, self.changed_at(e).unwrap_or_default(), self.get(e).unwrap())).collect();
        ron::to_string(&rows).map_err(|e| format!("{}: {e}", T::NAME))
    }
    fn restore_ron(&mut self, text: &str) -> Result<(), String> {
        let rows: Vec<(EntityId, SimTime, T)> = ron::from_str(text).map_err(|e| format!("{}: {e}", T::NAME))?;
        *self = SparseSet::new();
        for (e, t, c) in rows { self.insert(e, c, t); }
        Ok(())
    }
    fn as_any(&self) -> &dyn Any { self }
    fn as_any_mut(&mut self) -> &mut dyn Any { self }
}

pub fn store_ctor<T: Component>() -> Box<dyn AnyStore> { Box::new(SparseSet::<T>::new()) }

/// Declare a component: a struct with reflected fields, registered under a kind NAME.
///
/// ```ignore
/// component! {
///     /// Temperature of a thing that has mass.
///     pub struct Thermal = "thermal" { pub t: Relax, pub mass_kg: f64 }
/// }
/// ```
/// Every field type must implement `FieldValue` (scalars, times, trajectories, ids, strings,
/// `value_enum!` enums; `Vec<T>`/`IndexMap` travel as opaque bytes).
#[macro_export]
macro_rules! component {
    (
        $(#[$meta:meta])*
        $vis:vis struct $name:ident = $kind:literal {
            $( $(#[$fmeta:meta])* $fvis:vis $field:ident : $ty:ty ),* $(,)?
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Debug, PartialEq, ::serde::Serialize, ::serde::Deserialize)]
        $vis struct $name { $( $(#[$fmeta])* $fvis $field : $ty ),* }
        impl $crate::component::Component for $name {
            const NAME: &'static str = $kind;
            const FIELDS: &'static [&'static str] = &[ $( stringify!($field) ),* ];
            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn field(&self, i: $crate::id::FieldId) -> Option<$crate::value::Value> {
                let mut n: u16 = 0;
                $( if i.0 == n { return Some($crate::value::FieldValue::to_value(&self.$field)); } n += 1; )*
                None
            }
            #[allow(unused_assignments, unused_mut, unused_variables)]
            fn set_field(&mut self, i: $crate::id::FieldId, v: $crate::value::Value) -> Result<(), $crate::value::ValueError> {
                let mut n: u16 = 0;
                $( if i.0 == n { self.$field = <$ty as $crate::value::FieldValue>::from_value(v)?; return Ok(()); } n += 1; )*
                Err($crate::value::ValueError { expected: "known field", got: "unknown field index" })
            }
        }
        impl $name {
            /// Field id by name (panics on a typo - that is a programming error, caught by tests).
            #[allow(dead_code)]
            pub fn f(name: &str) -> $crate::id::FieldId {
                <$name as $crate::component::Component>::field_id(name).unwrap_or_else(|| panic!("component {} has no field {}", $kind, name))
            }
        }
    };
}
