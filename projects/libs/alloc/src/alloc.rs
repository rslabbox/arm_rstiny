//! Userland general-purpose heap allocator (interpreter-app.md decision B).
//!
//! A first-fit free list with boundary-tag coalescing (forward and backward),
//! exposed both as a Rust API and as the C ABI `malloc`/`calloc`/`realloc`/
//! `free`/`malloc_usable_size` symbols a C rt0 can link. Single-core, no
//! locks, no recursion into the kernel beyond growing the heap.
//!
//! Memory comes from the task's own budget:
//! - a static bootstrap pool in `.bss` (itself mapped by the ELF loader out of
//!   the task's sub-Untyped), and
//! - pages grown on demand through the kernel `Runtime::Map` service, which
//!   accounts frames against the same budget. No new kernel mechanism.
//!
//! Block layout (size always 16-aligned, the low bit of `size_used` is the
//! "in use" flag):
//!
//! ```text
//!          ┌──────────┬────────┬──────────┬──────────┬──────────────┐
//!  used:   │ prev_size│ size 1 │ payload  │          │              │
//!  free:   │ prev_size│ size 0 │ next     │ prev     │ payload      │
//!          └──────────┴────────┴──────────┴──────────┴──────────────┘
//! ```
//!
//! `prev_size` lets `free` find the previous block for backward coalescing
//! without a footer; the segment table decides whether a neighbour address is
//! a real block inside one of the heap's segments (the static pool or a grown
//! page run).

#![no_std]

use core::ptr;

use kernel_abi as abi;

/// Static bootstrap pool carved from `.bss` by the ELF loader.
pub const ALLOC_POOL_BYTES: usize = 32 * 1024;
/// First virtual address the allocator grows into with `Runtime::Map`.
///
/// Each task owns its own address space, so this absolute VA is fine as long
/// as the application does not map over it (none of the shipped apps do).
pub const ALLOC_GROW_VA: usize = 0x0780_0000;
/// Total heap ceiling: pool plus grown pages. ~256 KiB covers the MicroPython
/// C heap budget (128 KiB) with headroom; adjust the constant per application.
pub const ALLOC_MAX_TOTAL: usize = 256 * 1024;
/// Bytes grown per `Runtime::Map` call; a page run keeps syscall pressure low.
pub const ALLOC_GROW_ROUND: usize = 16 * 1024;
/// Maximum number of grown segments (each round appends one segment).
pub const ALLOC_MAX_SEGMENTS: usize = 16;

const USED: usize = 1;
const HDR: usize = 16; // prev_size + size_used
const LINK: usize = 16; // next + prev stored in a free block's payload
const MIN_BLOCK: usize = 48; // HDR + LINK + 16 aligned minimum payload

#[repr(align(16))]
struct Pool([u8; ALLOC_POOL_BYTES]);

static mut POOL: Pool = Pool([0; ALLOC_POOL_BYTES]);
/// Head of the free list (a block address, 0 = empty). Single-core: no lock.
static mut FREE_HEAD: usize = 0;
/// Segment table: `(start, len)` for the pool and every grown page run.
static mut SEGMENTS: [(usize, usize); ALLOC_MAX_SEGMENTS + 1] = [(0, 0); ALLOC_MAX_SEGMENTS + 1];
static mut SEGMENT_COUNT: usize = 0;
/// Next VA to grow into, and how much of the ceiling is already committed.
static mut GROW_NEXT: usize = ALLOC_GROW_VA;
static mut GROWN_BYTES: usize = 0;
/// Cached slot of this task's TCB capability (Runtime::Current), for maps.
static mut CURRENT_TCB: usize = 0;

/// One allocation unit. `size_used & !1` is the block size (header included),
/// `size_used & 1` reports whether the block is in use.
#[repr(C)]
struct Block {
    prev_size: usize,
    size_used: usize,
}

// ---- low-level helpers ----------------------------------------------------

unsafe fn block_at(addr: usize) -> *mut Block {
    addr as *mut Block
}

unsafe fn size_of(addr: usize) -> usize {
    (*block_at(addr)).size_used & !USED
}

unsafe fn used_of(addr: usize) -> bool {
    (*block_at(addr)).size_used & USED != 0
}

/// Payload pointer of a block (`addr + HDR`, 16-aligned).
unsafe fn payload(addr: usize) -> *mut u8 {
    (addr + HDR) as *mut u8
}

/// Block address a payload pointer belongs to.
unsafe fn block_of(payload: *mut u8) -> usize {
    (payload as usize) - HDR
}

/// Whether `addr` lies inside one of the heap's segments (i.e. is a block
/// header position, not arbitrary memory past the last block).
unsafe fn in_segments(addr: usize) -> bool {
    for index in 0..*ptr::addr_of!(SEGMENT_COUNT) {
        let (start, len) = (ptr::addr_of!(SEGMENTS)).read_volatile()[index];
        if addr >= start && addr < start + len {
            return true;
        }
    }
    false
}

unsafe fn extended_total() -> usize {
    ALLOC_POOL_BYTES + *ptr::addr_of!(GROWN_BYTES)
}

/// Grow the heap by mapping fresh zero-filled pages and append the run to the
/// segment table. Returns the new segment start (0 on failure).
unsafe fn grow(need: usize) -> usize {
    if *ptr::addr_of!(SEGMENT_COUNT) >= ALLOC_MAX_SEGMENTS {
        return 0;
    }
    let total = extended_total().saturating_add(need.max(ALLOC_GROW_ROUND));
    if total > ALLOC_MAX_TOTAL {
        return 0;
    }
    // Round up to the page; the map service maps page runs only.
    let length = (need.max(ALLOC_GROW_ROUND) + 4095) & !4095;
    let va = *ptr::addr_of!(GROW_NEXT);
    // Runtime::Map(target, va, len, rights=3 R|W) on this task's own VSpace.
    if runtime_map(va, length).is_err() {
        return 0;
    }
    ptr::addr_of_mut!(GROW_NEXT).write_volatile(va + length);
    ptr::addr_of_mut!(GROWN_BYTES).write_volatile(*ptr::addr_of!(GROWN_BYTES) + length);
    let segments = ptr::addr_of_mut!(SEGMENTS);
    let count = *ptr::addr_of!(SEGMENT_COUNT);
    (*segments).write_volatile(from_raw_parts_mut_array(count, (va, length)));
    ptr::addr_of_mut!(SEGMENT_COUNT).write_volatile(count + 1);
    // The run is one free block (leading block header + payload tail).
    let first = block_at(va);
    (*first).prev_size = 0;
    (*first).size_used = length;
    // Link it: new segments are adjacent in VA, so merge with the previous
    // segment's tail if that tail is a free block.
    if count > 0 {
        let previous = (*segments).read_volatile()[count - 1];
        let tail = previous.0 + previous.1;
        if tail == va && !used_of(tail - size_of(tail - size_of(tail))) {
            // Tail's block starts at `tail - prev_size`; recompute cleanly.
            let tail_block = tail - size_of_prev(tail);
            if !used_of(tail_block) && tail_block + size_of(tail_block) == va {
                // Already free: unlink and enlarge instead of double-linking.
                unlink(tail_block);
                (*first).prev_size = (*block_at(tail_block)).prev_size;
                (*block_at(tail_block)).size_used = size_of(tail_block) + length;
                // The merged block spans tail_block..va+length; note the two
                // headers differ, so rewrite the head in place is wrong — keep
                // the leading block of the *tail* as the merged head.
                (*first).size_used = 1; // mark the (now interior) head used
                push_free(tail_block);
                return va;
            }
        }
    }
    push_free(va);
    va
}

/// prev_size of the block whose header starts at `addr`.
unsafe fn size_of_prev(addr: usize) -> usize {
    (*block_at(addr)).prev_size
}

/// Unlink a free block from the free list (blob validation by caller).
unsafe fn unlink(addr: usize) {
    let block = block_at(addr);
    let next = (*block).next();
    let prev = (*block).prev();
    if prev != 0 {
        (*(block_at(prev))).set_next(next);
    } else {
        ptr::addr_of_mut!(FREE_HEAD).write_volatile(next);
    }
    if next != 0 {
        (*(block_at(next))).set_prev(prev);
    }
}

/// Push a free block at the head of the free list (its payload already
/// reserved for the link fields).
unsafe fn push_free(addr: usize) {
    let head = *ptr::addr_of!(FREE_HEAD);
    let block = block_at(addr);
    (*block).set_next(head);
    (*block).set_prev(0);
    if head != 0 {
        (*(block_at(head))).set_prev(addr);
    }
    ptr::addr_of_mut!(FREE_HEAD).write_volatile(addr);
}

/// First-fit allocation from the existing free list. Returns the block address
/// with the block marked used (header untouched further); 0 when no block fits.
unsafe fn take_free(need: usize) -> usize {
    let mut current = *ptr::addr_of!(FREE_HEAD);
    while current != 0 {
        let size = size_of(current);
        debug_assert!(!used_of(current));
        if size >= need {
            unlink(current);
            return current;
        }
        current = (*block_at(current)).next();
    }
    0
}

/// Allocate `need` (16-aligned block size) from freelist or a fresh segment.
unsafe fn alloc_block(need: usize) -> usize {
    let block = take_free(need);
    if block != 0 {
        return block;
    }
    let grown = grow(need);
    if grown == 0 {
        return 0;
    }
    // The just-inserted segment root is free at the list head; take it.
    let head = *ptr::addr_of!(FREE_HEAD);
    if head == 0 {
        return 0;
    }
    let head = take_free(need);
    if head != 0 {
        return head;
    }
    // Segment root was merged into a larger neighbour: first-fit again.
    take_free(need)
}

// ---- Block helpers for alloc/free paths -----------------------------------

impl Block {
    unsafe fn next(&self) -> usize {
        ptr::read_volatile((self as *const Block).cast::<u8>().add(HDR).cast::<usize>())
    }
    unsafe fn set_next(&mut self, value: usize) {
        ptr::write_volatile((self as *mut Block).cast::<u8>().add(HDR).cast::<usize>(), value);
    }
    unsafe fn prev(&self) -> usize {
        ptr::read_volatile(
            (self as *const Block).cast::<u8>().add(HDR + 8).cast::<usize>(),
        )
    }
    unsafe fn set_prev(&mut self, value: usize) {
        ptr::write_volatile(
            (self as *mut Block).cast::<u8>().add(HDR + 8).cast::<usize>(),
            value,
        );
    }
}

/// Mark `block` used with size `size`; the payload is zero-initialised only
/// for `calloc`. Splits oversized blocks so the tail returns to the free list.
unsafe fn commit_used(mut block: usize, mut size: usize, need: usize) -> usize {
    debug_assert!(size >= need);
    if size - need >= MIN_BLOCK {
        let left = need;
        let right = block + need;
        let right_block = block_at(right);
        (*right_block).prev_size = left;
        (*right_block).size_used = size - need;
        push_free(right);
        size = left;
    }
    (*(block_at(block))).prev_size = if block == begin_of_segment(block) { 0 } else { block_prev_size(block) };
    (*block_at(block)).size_used = size | USED;
    block
}

/// prev_size to record on a block about to become used: the size of its
/// immediate predecessor block (or 0 at a segment start).
unsafe fn block_prev_size(block: usize) -> usize {
    for index in 0..*ptr::addr_of!(SEGMENT_COUNT) {
        let (start, _) = (ptr::addr_of!(SEGMENTS)).read_volatile()[index];
        if block == start {
            return 0;
        }
    }
    // The predecessor ends exactly at `block`: it is the nearest block whose
    // end address is `block`. Its size equals `block - predecessor_start`, but
    // we don't track block starts; instead the predecessor's header is found
    // through `block - predecessor_size`. Reconstruct it iteratively: the
    // predecessor is the previous block in address order, whose size we read
    // from... its own header. To avoid a walk, cache: any previously used
    // block wrote prev_size into the successor. So read the *next* block's
    // prev_size? No — the block following `block` stores `block`'s size only
    // if it is free/used with that field. Fall back to a linear walk.
    prev_block_size(block)
}

/// Size of the block immediately before `block` (address order), found by
/// scanning blocks from the containing segment start. Segments are short.
unsafe fn prev_block_size(block: usize) -> usize {
    for index in 0..*ptr::addr_of!(SEGMENT_COUNT) {
        let (start, len) = (ptr::addr_of!(SEGMENTS)).read_volatile()[index];
        if block > start && block < start + len {
            let mut cursor = start;
            while cursor < block {
                let size = size_of(cursor);
                if size == 0 || cursor + size > block {
                    return 0; // corrupt or empty space: treat as no predecessor
                }
                if cursor + size == block {
                    return size;
                }
                cursor += size;
            }
        }
    }
    0
}

unsafe fn begin_of_segment(block: usize) -> usize {
    for index in 0..*ptr::addr_of!(SEGMENT_COUNT) {
        let (start, _) = (ptr::addr_of!(SEGMENTS)).read_volatile()[index];
        if block == start {
            return start;
        }
    }
    0
}

// from_raw_parts_mut_array helper to keep the volatile segment write tidy.
#[allow(clippy::mut_from_ref)]
unsafe fn from_raw_parts_mut_array(slot: usize, value: (usize, usize)) -> (usize, usize) {
    let segments = ptr::addr_of_mut!(SEGMENTS);
    let mut array = (*segments).read_volatile();
    array[slot] = value;
    array
}