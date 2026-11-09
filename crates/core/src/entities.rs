//! Generational entity allocator. Reservation is by `&self` (handlers hold `&World`) and is
//! committed or released by the engine when the spawn proposal is applied or rejected.
use crate::id::EntityId;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;

#[derive(Clone, Default, Serialize, Deserialize)]
struct Inner {
    gens: Vec<u32>,
    /// 0 free, 1 alive, 2 reserved
    state: Vec<u8>,
    free: Vec<u32>,
}

#[derive(Default)]
pub struct Entities { inner: RefCell<Inner> }

impl Entities {
    pub fn new() -> Entities { Entities::default() }
    pub fn alive(&self, e: EntityId) -> bool {
        let i = self.inner.borrow();
        let ix = e.index as usize;
        ix < i.gens.len() && i.state[ix] == 1 && i.gens[ix] == e.gen
    }
    pub fn reserved(&self, e: EntityId) -> bool {
        let i = self.inner.borrow();
        let ix = e.index as usize;
        ix < i.gens.len() && i.state[ix] == 2 && i.gens[ix] == e.gen
    }
    /// Reserve an id (not alive yet). Deterministic: LIFO free list, then append.
    pub fn reserve(&self) -> EntityId {
        let mut i = self.inner.borrow_mut();
        if let Some(ix) = i.free.pop() {
            i.state[ix as usize] = 2;
            EntityId { index: ix, gen: i.gens[ix as usize] }
        } else {
            let ix = i.gens.len() as u32;
            i.gens.push(0);
            i.state.push(2);
            EntityId { index: ix, gen: 0 }
        }
    }
    pub fn commit(&self, e: EntityId) -> bool {
        let mut i = self.inner.borrow_mut();
        let ix = e.index as usize;
        if ix < i.gens.len() && i.state[ix] == 2 && i.gens[ix] == e.gen { i.state[ix] = 1; true } else { false }
    }
    pub fn release(&self, e: EntityId) {
        let mut i = self.inner.borrow_mut();
        let ix = e.index as usize;
        if ix < i.gens.len() && i.state[ix] == 2 && i.gens[ix] == e.gen {
            i.state[ix] = 0;
            i.free.push(e.index);
        }
    }
    pub fn alloc(&self) -> EntityId {
        let e = self.reserve();
        self.commit(e);
        e
    }
    /// Free an alive entity; its generation advances so stale ids never resolve.
    pub fn free(&self, e: EntityId) -> bool {
        let mut i = self.inner.borrow_mut();
        let ix = e.index as usize;
        if ix < i.gens.len() && i.state[ix] == 1 && i.gens[ix] == e.gen {
            i.state[ix] = 0;
            i.gens[ix] = i.gens[ix].wrapping_add(1);
            i.free.push(e.index);
            true
        } else { false }
    }
    pub fn iter_alive(&self) -> Vec<EntityId> {
        let i = self.inner.borrow();
        (0..i.gens.len()).filter(|&ix| i.state[ix] == 1).map(|ix| EntityId { index: ix as u32, gen: i.gens[ix] }).collect()
    }
    pub fn count_alive(&self) -> usize { self.inner.borrow().state.iter().filter(|s| **s == 1).count() }
    pub fn capacity(&self) -> usize { self.inner.borrow().gens.len() }
    pub fn hash_into(&self, h: &mut blake3::Hasher) {
        let i = self.inner.borrow();
        h.update(b"entities");
        h.update(&(i.gens.len() as u64).to_le_bytes());
        for ix in 0..i.gens.len() {
            h.update(&i.gens[ix].to_le_bytes());
            h.update(&[i.state[ix]]);
        }
    }
    pub fn snapshot(&self) -> Vec<u8> { postcard::to_stdvec(&*self.inner.borrow()).expect("postcard") }
    pub fn restore(&self, bytes: &[u8]) -> Result<(), String> {
        let inner: Inner = postcard::from_bytes(bytes).map_err(|e| e.to_string())?;
        *self.inner.borrow_mut() = inner;
        Ok(())
    }
}
