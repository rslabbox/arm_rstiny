//! Userland general-purpose heap allocator (interpreter-app.md decision B).
//!
//! A first-fit free list with boundary-tag coalescing (forward and backward),
//! exposed both as a Rust API and as the C ABI `malloc`/`calloc`/`realloc`/
//! `free`/`malloc_usable_size` symbols a C rt0 (e.g. the MicroPython port)
//! links directly.
//!
//! Memory comes from the task's own budget:
//! - a static bootstrap pool in `.bss` (mapped by the ELF loader out of the
//!   task's sub-Untyped), and
//! - pages grown on demand through the kernel `Runtime::Map` service, which
//!   accounts the frames against the same budget (no new kernel mechanism).
//!
//! Block layout (16-byte aligned size; the low bit of the size word is the
//! "in use" flag):
//!
//! ```text
//!   B ┌─────────────┬───────────────────────────────┬────────────┐
//!     │ size|used 8 │ padding 8                     │ footer     │  size 16
//!     ├─────────────┼───────────────────────────────┤            │
//!  payload: free → next, prev, then user bytes      ├────────────┤
//!           used → user bytes                       │ size|used 8 │
//!   B+size-8 └──────────────────────────────────────┴────────────┘
//! ```
//!
//! Every block (free or used) carries the footer, so `free` finds the
//! previous block's size and state without a `prev_size` field that could go
//! stale across splits. The segment table decides whether a neighbour address
//! is a real block header inside one of the heap's segments; segments are the
//! static pool and each grown page run (segments are independent, no
//! cross-segment coalescing).

#![no_std]
#![allow(unsafe_op_in_unsafe_fn)] // single-core heap internals are fully unsafe

use core::ptr;

use kernel_abi as abi;

/// Static bootstrap pool carved from `.bss` by the ELF loader.
pub const ALLOC_POOL_BYTES: usize = 32 * 1024;
/// First virtual address the allocator grows into with `Runtime::Map`.
/// Each task owns its own address space, so this absolute VA is fine unless
/// the application maps over it (none of the shipped apps do).
pub const ALLOC_GROW_VA: usize = 0x0780_0000;
/// Heap ceiling: pool plus grown pages. ~256 KiB covers the MicroPython C heap
/// budget (128 KiB) with headroom; tune per application.
/// Cap on the total heap (pool + grown runs). 512 KiB fits the debug-mode
/// C app images (rstiny-alloc staticlib carries debug symbols) inside the
/// heap budget the supervisor grants; release images stay far below.
pub const ALLOC_MAX_TOTAL: usize = 1024 * 1024;
/// Bytes grown per `Runtime::Map` call: a page run keeps syscall pressure low.
pub const ALLOC_GROW_ROUND: usize = 16 * 1024;
/// Maximum grown segments (each round appends exactly one).
pub const ALLOC_MAX_SEGMENTS: usize = 16;

const USED: usize = 1;
/// Header overhead: [size|used : 8][padding : 8].
const HDR: usize = 16;
/// Footer: [size|used : 8].
const FTR: usize = 8;
/// Free-block link fields inside the payload: [next : 8][prev : 8].
const LINK: usize = 16;
/// Smallest block that can exist (must be able to hold the links).
const MIN_BLOCK: usize = HDR + LINK + FTR + 8; // 48; payload ≥ 8
/// Bytes consumed by header + footer of every block.
const OVERHEAD: usize = HDR + FTR;

#[allow(dead_code)] // storage for alignment; only its address is used
#[repr(align(16))]
struct Pool([u8; ALLOC_POOL_BYTES]);

#[allow(dead_code)] // storage for alignment; only its address is used
static mut POOL: Pool = Pool([0; ALLOC_POOL_BYTES]);
/// Head of the free list (a block address, 0 = empty).
static mut FREE_HEAD: usize = 0;
/// One-time bootstrap (first `malloc` sets up the pool).
static mut INITIALIZED: bool = false;
/// Segment table: one entry for the pool, one per grown page run.
static mut SEGMENTS: [(usize, usize); ALLOC_MAX_SEGMENTS + 1] = [(0, 0); ALLOC_MAX_SEGMENTS + 1];
static mut SEGMENT_COUNT: usize = 0;
/// Next VA to grow into and how much of the ceiling is already committed.
static mut GROW_NEXT: usize = ALLOC_GROW_VA;
static mut GROWN_BYTES: usize = 0;
/// Cached slot of this task's TCB capability (from Runtime::Current), used as
/// the Runtime::Map target; resolved once.
static mut CURRENT_TCB: usize = 0;

// ---- block primitives -----------------------------------------------------

#[inline]
unsafe fn size_of(addr: usize) -> usize {
    (*(addr as *const usize)) & !USED
}

#[inline]
unsafe fn used_of(addr: usize) -> bool {
    (*(addr as *const usize)) & USED != 0
}

#[inline]
unsafe fn set_size_used(addr: usize, size: usize, used: bool) {
    *(addr as *mut usize) = size | usize::from(used);
}

/// Footer address of the block starting at `addr`.
#[inline]
#[allow(dead_code)]
unsafe fn footer(addr: usize) -> usize {
    addr + size_of(addr) - FTR
}

/// Header address of the block ending at `addr`.
#[inline]
#[allow(dead_code)]
unsafe fn header_of_footer(footer_addr: usize) -> usize {
    footer_addr + FTR - size_of(footer_addr)
}

/// Whether `addr` lies inside one of the heap's segments.
#[inline]
unsafe fn in_segments(addr: usize) -> bool {
    let count = ptr::addr_of!(SEGMENT_COUNT).read();
    let segments = ptr::addr_of!(SEGMENTS).read();
    for segment in segments.iter().take(count) {
        if addr >= segment.0 && addr < segment.0 + segment.1 {
            return true;
        }
    }
    false
}

/// User payload pointer of a block (block address + header size).
#[inline]
unsafe fn payload(addr: usize) -> *mut u8 {
    (addr + HDR) as *mut u8
}

/// Block address a user pointer belongs to.
#[inline]
unsafe fn block_of(pointer: *mut u8) -> usize {
    pointer as usize - HDR
}

// ---- free-list operations -------------------------------------------------

unsafe fn free_next(addr: usize) -> usize {
    *(addr as *const usize).add(HDR / 8)
}
unsafe fn set_free_next(addr: usize, value: usize) {
    *(addr as *mut usize).add(HDR / 8) = value;
}
unsafe fn free_prev(addr: usize) -> usize {
    *(addr as *const usize).add(HDR / 8 + 1)
}
unsafe fn set_free_prev(addr: usize, value: usize) {
    *(addr as *mut usize).add(HDR / 8 + 1) = value;
}

/// Unlink a free block from the free list.
unsafe fn unlink(addr: usize) {
    let next = free_next(addr);
    let prev = free_prev(addr);
    if prev != 0 {
        set_free_next(prev, next);
    } else {
        ptr::addr_of_mut!(FREE_HEAD).write(next);
    }
    if next != 0 {
        set_free_prev(next, prev);
    }
}

/// Push a free block at the head of the free list.
unsafe fn push_free(addr: usize) {
    let head = ptr::addr_of!(FREE_HEAD).read();
    set_free_next(addr, head);
    set_free_prev(addr, 0);
    if head != 0 {
        set_free_prev(head, addr);
    }
    ptr::addr_of_mut!(FREE_HEAD).write(addr);
}

/// First-fit: return the address of a free block of at least `need` bytes and
/// unlink it. Returns 0 when nothing fits.
unsafe fn take_free(need: usize) -> usize {
    let mut current = ptr::addr_of!(FREE_HEAD).read();
    while current != 0 {
        debug_assert_eq!(used_of(current), false);
        if size_of(current) >= need {
            unlink(current);
            return current;
        }
        current = free_next(current);
    }
    0
}

// ---- growth ---------------------------------------------------------------

/// Runtime::Map call: `map_vspace(current_task, va, len, R|W)`; frames are
/// accounted against the task's budget untrusted region.
unsafe fn runtime_map(va: usize, len: usize) -> Result<(), ()> {
    let tcb = if ptr::addr_of!(CURRENT_TCB).read() != 0 {
        ptr::addr_of!(CURRENT_TCB).read()
    } else {
        let reply = call(
            abi::INIT_RUNTIME,
            abi::RuntimeInvocation::Current as u64,
            [0; 4],
            0,
        );
        let label = reply.0 >> 12 & 0xF_FFFF_FFFF_FFFF;
        if label != abi::OK {
            return Err(());
        }
        let slot = reply.1 as usize;
        ptr::addr_of_mut!(CURRENT_TCB).write(slot);
        slot
    };
    let reply = call(
        abi::INIT_RUNTIME,
        abi::RuntimeInvocation::Map as u64,
        [tcb as u64, va as u64, len as u64, 3],
        4,
    );
    if reply.0 >> 12 & 0xF_FFFF_FFFF_FFFF == abi::OK {
        Ok(())
    } else {
        Err(())
    }
}

/// Map one fresh page run and insert it as a free block and a new segment.
/// Returns 0 on failure.
unsafe fn grow(need: usize) -> usize {
    let count = ptr::addr_of!(SEGMENT_COUNT).read();
    if count > ALLOC_MAX_SEGMENTS {
        return 0;
    }
    let ceiling = ALLOC_POOL_BYTES + ptr::addr_of!(GROWN_BYTES).read();
    // Round up to the page; the map service maps page runs only and the
    // 41 KiB-style requests (40 KiB argv allocation) must not be rejected.
    let length = (need.max(ALLOC_GROW_ROUND) + 4095) & !4095;
    if ceiling + length > ALLOC_MAX_TOTAL || length % 4096 != 0 {
        return 0;
    }
    let va = ptr::addr_of!(GROW_NEXT).read();
    if runtime_map(va, length).is_err() {
        return 0;
    }
    ptr::addr_of_mut!(GROW_NEXT).write(va + length);
    ptr::addr_of_mut!(GROWN_BYTES).write(ceiling - ALLOC_POOL_BYTES + length);
    let segments = ptr::addr_of_mut!(SEGMENTS);
    let mut table = segments.read();
    table[count] = (va, length);
    segments.write(table);
    ptr::addr_of_mut!(SEGMENT_COUNT).write(count + 1);
    // The run is one free block spanning the whole segment.
    set_size_used(va, length, false);
    set_size_used(va + length - FTR, length, false);
    push_free(va);
    va
}

/// Allocate `need` (block size, 16-aligned) from the free list, growing the
/// heap first when nothing fits. Returns the block address or 0.
unsafe fn alloc_block(need: usize) -> usize {
    let mut block = take_free(need);
    if block == 0 {
        if grow(need) == 0 {
            return 0;
        }
        // The fresh segment is the newest free block; first-fit again.
        block = take_free(need);
        if block == 0 {
            return 0;
        }
    }
    let size = size_of(block);
    if size - need >= MIN_BLOCK {
        // Split oversized blocks so the tail returns to the free list.
        let right = block + need;
        let right_size = size - need;
        set_size_used(right, right_size, false);
        set_size_used(right + right_size - FTR, right_size, false);
        push_free(right);
        set_size_used(block, need, true);
        set_size_used(block + need - FTR, need, true);
    } else {
        set_size_used(block, size, true);
        set_size_used(block + size - FTR, size, true);
    }
    block
}

// ---- public allocation API ------------------------------------------------

/// Allocate `size` usable bytes, 16-byte aligned. Returns null on failure.
///
/// # Safety
/// The returned pointer is uninitialized; Rust callers must treat it as such.
pub unsafe fn alloc(size: usize) -> *mut u8 {
    unsafe {
        ensure_initialized();
        let need = (size + OVERHEAD).max(MIN_BLOCK).next_multiple_of(16);
        let block = alloc_block(need);
        if block == 0 {
            ptr::null_mut()
        } else {
            payload(block)
        }
    }
}

/// Deallocate a pointer from [`alloc`] (or a C `malloc`).
///
/// # Safety
/// `pointer` must come from this allocator and not be freed before.
pub unsafe fn dealloc(pointer: *mut u8) {
    if pointer.is_null() {
        return;
    }
    unsafe {
        let mut block = block_of(pointer);
        if !used_of(block) {
            return; // double free: ignore rather than corrupt the list
        }
        let mut size = size_of(block);
        set_size_used(block, size, false);
        set_size_used(block + size - FTR, size, false);
        // Backward coalescing: the previous block's footer sits right before
        // this header, carrying the previous block's total size.
        if in_segments(block - FTR) && !used_of(block - FTR) {
            let prev = block - size_of(block - FTR);
            unlink(prev);
            size += size_of(prev);
            block = prev;
        }
        // Forward coalescing: the next block starts right after this one.
        let next = block + size;
        if in_segments(next) && !used_of(next) {
            unlink(next);
            size += size_of(next);
        }
        set_size_used(block, size, false);
        set_size_used(block + size - FTR, size, false);
        push_free(block);
    }
}

/// Usable bytes at `pointer` (C `malloc_usable_size`).
///
/// # Safety
/// `pointer` must come from this allocator.
pub unsafe fn usable(pointer: *mut u8) -> usize {
    if pointer.is_null() {
        return 0;
    }
    unsafe { size_of(block_of(pointer)) - OVERHEAD }
}

/// Grow or shrink a live allocation, copying contents across when it moves.
///
/// # Safety
/// `pointer` must come from this allocator and not be freed before.
pub unsafe fn reallocate(pointer: *mut u8, size: usize) -> *mut u8 {
    if pointer.is_null() {
        return unsafe { alloc(size) };
    }
    let old = unsafe { usable(pointer) };
    if old >= size {
        // Enough space already; no split (kept simple, see module docs).
        return pointer;
    }
    let new = unsafe { alloc(size) };
    if new.is_null() {
        return ptr::null_mut();
    }
    unsafe {
        ptr::copy_nonoverlapping(pointer, new, old.min(size));
        dealloc(pointer);
    }
    new
}

// ---- bootstrap ------------------------------------------------------------

unsafe fn ensure_initialized() {
    if ptr::addr_of!(INITIALIZED).read() {
        return;
    }
    let pool = ptr::addr_of_mut!(POOL) as *mut u8 as usize;
    if pool % 16 != 0 {
        // Paranoia: `repr(align(16))` guarantees 16-byte alignment.
        unreachable!("pool is 16-byte aligned by construction");
    }
    ptr::addr_of_mut!(INITIALIZED).write(true);
    ptr::addr_of_mut!(FREE_HEAD).write(pool);
    // The whole pool starts as one free block; its footer closes the last
    // 8 bytes of `.bss`.
    set_size_used(pool, ALLOC_POOL_BYTES, false);
    set_size_used(pool + ALLOC_POOL_BYTES - FTR, ALLOC_POOL_BYTES, false);
    set_free_next(pool, 0);
    set_free_prev(pool, 0);
    let segments = ptr::addr_of_mut!(SEGMENTS);
    let mut table = segments.read();
    table[0] = (pool, ALLOC_POOL_BYTES);
    segments.write(table);
    ptr::addr_of_mut!(SEGMENT_COUNT).write(1);
}

// ---- kernel invocation (svc #0, Call) -------------------------------------

/// Fire one `svc #0` Call on `cap` with `word_len` message words (0..=4) and
/// return (reply tag, mr0). Words beyond four would need the IPC buffer; the
/// allocator only ever sends four.
unsafe fn call(cap: u64, label: u64, words: [u64; 4], word_len: u64) -> (u64, u64) {
    let mut tag = (label << 12) | word_len;
    let mut mr0 = words[0];
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x7") abi::Syscall::Call as i64 as u64,
            in("x0") cap,
            inlateout("x1") tag,
            inlateout("x2") mr0,
            in("x3") words[1],
            in("x4") words[2],
            in("x5") words[3],
        );
    }
    (tag, mr0)
}

// ---- C ABI ----------------------------------------------------------------

/// C `malloc(size_t) -> void *`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc(size: usize) -> *mut u8 {
    unsafe { alloc(size) }
}

/// C `calloc(nmemb, size) -> void *`; zero-initialised.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn calloc(num: usize, size: usize) -> *mut u8 {
    let Some(total) = num.checked_mul(size) else {
        return ptr::null_mut();
    };
    let pointer = unsafe { alloc(total) };
    if !pointer.is_null() {
        unsafe {
            ptr::write_bytes(pointer, 0, total);
        }
    }
    pointer
}

/// C `realloc(ptr, size) -> void *`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn realloc(pointer: *mut u8, size: usize) -> *mut u8 {
    if size == 0 {
        unsafe { dealloc(pointer) };
        return ptr::null_mut();
    }
    unsafe { reallocate(pointer, size) }
}

/// C `free(void *)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn free(pointer: *mut u8) {
    unsafe { dealloc(pointer) };
}

/// C `malloc_usable_size(void *) -> size_t`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn malloc_usable_size(pointer: *mut u8) -> usize {
    unsafe { usable(pointer) }
}

/// The allocator runs inside C programs that have no Rust panic machinery of
/// their own; a logic bug here must not unwind into foreign code. Rust tasks
/// bring their own #[panic_handler] through rstiny-runtime, so this one is
/// only compiled into the C staticlib build (feature `c-heap-panic`).
#[cfg(feature = "c-heap-panic")]
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
