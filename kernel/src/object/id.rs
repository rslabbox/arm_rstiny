//! Bounded, generation-checked kernel object table.
//!
//! The table is the single owner of every kernel object payload. Capabilities
//! and address spaces only carry [`ObjectId`] values; they never own object
//! memory. Reusing a slot bumps its generation so a stale identifier can never
//! alias the new occupant (ABA protection).
use super::Object;
use alloc::collections::BTreeMap;

/// Hard upper bound on simultaneously live objects. Object memory is kernel
/// metadata, not user-controllable; exhaustion is reported, never panicked.
pub const MAX_OBJECTS: usize = 4096;

/// Stable identity of an object table entry.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct ObjectId {
    index: u32,
    generation: u32,
}

/// Which Untyped region backed an object and where inside it. Objects created
/// from kernel metadata (boot objects, TCBs) carry no owner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ObjectOwner {
    pub untyped: ObjectId,
    pub offset: usize,
    pub size: usize,
}

struct Slot {
    generation: u32,
    next_free: u32,
    object: Option<Object>,
    owner: Option<ObjectOwner>,
}

/// Single-owner object storage. Every access validates the generation, so a
/// capability naming a collected object resolves to `None` instead of aliasing
/// a freshly allocated object in the same slot.
pub struct ObjectTable {
    slots: BTreeMap<u32, Slot>,
    free: Option<u32>,
    next_index: u32,
    live: usize,
}
impl ObjectTable {
    pub const fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
            free: None,
            next_index: 0,
            live: 0,
        }
    }
    pub const fn len(&self) -> usize {
        self.live
    }
    pub const fn is_full(&self) -> bool {
        self.live >= MAX_OBJECTS
    }
    /// Publish a payload and return its identity. On failure the payload is
    /// dropped, which returns any owned frame to its region or the boot pool.
    pub fn insert(&mut self, object: Object) -> Option<ObjectId> {
        self.insert_owned(object, None)
    }
    /// Publish a payload backed by `owner`'s Untyped region.
    pub fn insert_owned(&mut self, object: Object, owner: Option<ObjectOwner>) -> Option<ObjectId> {
        if self.is_full() {
            return None;
        }
        let index = match self.free {
            Some(index) => {
                let slot = self.slots.get_mut(&index).expect("free slot chain");
                self.free = (slot.next_free != u32::MAX).then_some(slot.next_free);
                index
            }
            None => {
                let index = self.next_index;
                self.next_index = self.next_index.checked_add(1)?;
                index
            }
        };
        let slot = self.slots.entry(index).or_insert(Slot {
            generation: 0,
            next_free: u32::MAX,
            object: None,
            owner: None,
        });
        slot.generation = slot.generation.wrapping_add(1);
        slot.next_free = u32::MAX;
        slot.object = Some(object);
        slot.owner = owner;
        self.live += 1;
        Some(ObjectId {
            index,
            generation: slot.generation,
        })
    }
    /// Remove and return the payload, releasing its ownership. A stale identity
    /// is rejected without disturbing the current occupant.
    pub fn remove(&mut self, id: ObjectId) -> Option<Object> {
        let slot = self.slots.get_mut(&id.index)?;
        if slot.generation != id.generation {
            return None;
        }
        let object = slot.object.take()?;
        slot.owner = None;
        slot.next_free = self.free.unwrap_or(u32::MAX);
        self.free = Some(id.index);
        self.live -= 1;
        Some(object)
    }
    pub fn get(&self, id: ObjectId) -> Option<&Object> {
        let slot = self.slots.get(&id.index)?;
        (slot.generation == id.generation)
            .then_some(slot.object.as_ref())
            .flatten()
    }
    pub fn get_mut(&mut self, id: ObjectId) -> Option<&mut Object> {
        let slot = self.slots.get_mut(&id.index)?;
        (slot.generation == id.generation)
            .then_some(slot.object.as_mut())
            .flatten()
    }
    /// The Untyped region backing an object, if any.
    pub fn owner(&self, id: ObjectId) -> Option<ObjectOwner> {
        let slot = self.slots.get(&id.index)?;
        if slot.generation != id.generation {
            return None;
        }
        slot.owner
    }
    /// Every object carved from `untyped`. First version scans the table; a
    /// full mapping database would replace this later.
    pub fn children(&self, untyped: ObjectId) -> alloc::vec::Vec<ObjectId> {
        self.iter()
            .filter_map(|(id, _)| (self.owner(id)?.untyped == untyped).then_some(id))
            .collect()
    }
    pub fn iter(&self) -> impl Iterator<Item = (ObjectId, &Object)> {
        self.slots.iter().filter_map(|(&index, slot)| {
            slot.object.as_ref().map(|object| {
                (
                    ObjectId {
                        index,
                        generation: slot.generation,
                    },
                    object,
                )
            })
        })
    }
    pub fn ids(&self) -> alloc::vec::Vec<ObjectId> {
        self.iter().map(|(id, _)| id).collect()
    }
}
