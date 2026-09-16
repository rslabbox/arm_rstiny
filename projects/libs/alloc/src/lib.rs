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
//! - pages grown on demand with standard object operations: `UntypedRetype`
//!   carves frames from the task's own Untyped budget cap and `Page_Map`
//!   installs them in the task's own VSpace. The budget's watermark is the
//!   single accounting point — there is no kernel convenience service and no
//!   global frame source (docs/capability-authority-untyped.md §3.2, C0).
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
/// First virtual address the allocator grows into. Each task owns its own
/// address space, so this absolute VA is fine unless the application maps
/// over it (none of the shipped apps do; the loader windows, ROM, MMIO and
/// thread groups all sit elsewhere).
pub const ALLOC_GROW_VA: usize = 0x0780_0000;
/// Heap ceiling: pool plus grown pages. ~256 KiB covers the MicroPython C heap
/// budget (128 KiB) with headroom; tune per application.
/// Cap on the total heap (pool + grown runs). 512 KiB fits the debug-mode
/// C app images (rstiny-alloc staticlib carries debug symbols) inside the
/// heap budget the supervisor grants; release images stay far below.
pub const ALLOC_MAX_TOTAL: usize = 1024 * 1024;
/// Bytes grown per page run: a run keeps syscall pressure low.
pub const ALLOC_GROW_ROUND: usize = 16 * 1024;
/// Maximum grown segments (each round appends exactly one).
pub const ALLOC_MAX_SEGMENTS: usize = 16;

/// Well-known capability slots of the *calling* task (same convention as the
/// user library). Every task in the service chain receives its private Untyped
/// budget in slot [`abi::INIT_UNTYPED`] and owns `INIT_CNODE`/`INIT_VSPACE`.
const BUDGET: u64 = abi::INIT_UNTYPED;
const CNODE: u64 = abi::INIT_CNODE;
const VSPACE: u64 = abi::INIT_VSPACE;
/// First CSpace slot the allocator retypes frames into. The slot cursor is
/// private to this task; the base sits above every documented window (loader
/// `LOADER_SLOT_BASE` + service strides, thread groups, service endpoints)
/// and leaves ~5.5k slots for the heap ceiling of 1 MiB (256 pages + table).
const SLOT_BASE: u64 = 60_000;

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
/// Next free CSpace slot for retyped frames, and whether the L3 table over
/// the grow window is already mapped. Heap pages are never unmapped, so both
/// only move forward.
static mut NEXT_SLOT: u64 = SLOT_BASE + 1;
static mut TABLE_MAPPED: bool = false;

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

/// Retype one object of `kind` from the task's budget into `slot`.
unsafe fn retype(kind: u64, slot: u64) -> Result<(), ()> {
    unsafe {
        invoke(
            BUDGET,
            abi::Invocation::UntypedRetype as u64,
            &[kind, 0, 0, 0, slot, 1],
            &[CNODE],
        )
        .map(|_| ())
    }
}

/// Map a retyped frame RW/NX at `va` in the task's own VSpace.
unsafe fn map_frame(slot: u64, va: usize) -> Result<(), ()> {
    unsafe {
        invoke(
            slot,
            abi::Invocation::ArmPageMap as u64,
            &[
                va as u64,
                abi::RIGHTS_READ | abi::RIGHTS_WRITE,
                abi::VM_CACHEABLE | abi::VM_EXECUTE_NEVER,
            ],
            &[VSPACE],
        )
        .map(|_| ())
    }
}

/// Delete a frame capability. Deletion unmaps the frame's recorded mapping
/// and lets the kernel reclaim the frame with its derivation subtree.
unsafe fn drop_frame(slot: u64) {
    unsafe {
        let _ = invoke(slot, abi::Invocation::CNodeDelete as u64, &[slot, 64], &[]);
    }
}

/// Map one fresh page run and insert it as a free block and a new segment.
/// Every frame is retyped from the task's own budget and mapped with standard
/// object operations; a failure deletes what this round created, leaving the
/// heap and the budget watermark untouched. Returns 0 on failure.
unsafe fn grow(need: usize) -> usize {
    let count = ptr::addr_of!(SEGMENT_COUNT).read();
    if count > ALLOC_MAX_SEGMENTS {
        return 0;
    }
    let ceiling = ALLOC_POOL_BYTES + ptr::addr_of!(GROWN_BYTES).read();
    // Round up to the page; large requests (40 KiB argv allocations) must not
    // be rejected for rounding.
    let length = (need.max(ALLOC_GROW_ROUND) + 4095) & !4095;
    if ceiling + length > ALLOC_MAX_TOTAL || length % 4096 != 0 {
        return 0;
    }
    let va = ptr::addr_of!(GROW_NEXT).read();
    // One L3 page table covers the whole grow window (ALLOC_GROW_VA is
    // 2 MiB aligned and ALLOC_MAX_TOTAL stays inside one 2 MiB region).
    if !ptr::addr_of!(TABLE_MAPPED).read() {
        if unsafe { retype(abi::ObjectType::PageTable as u64, SLOT_BASE) }.is_err() {
            return 0;
        }
        // SAFETY: a freshly retyped table in this task's own address space.
        if unsafe {
            invoke(
                SLOT_BASE,
                abi::Invocation::ArmPageTableMap as u64,
                &[ALLOC_GROW_VA as u64, abi::VM_CACHEABLE],
                &[VSPACE],
            )
        }
        .is_err()
        {
            unsafe { drop_frame(SLOT_BASE) };
            return 0;
        }
        ptr::addr_of_mut!(TABLE_MAPPED).write(true);
    }
    let mut mapped: u64 = 0;
    for index in 0..(length / 4096) {
        let slot = ptr::addr_of!(NEXT_SLOT).read();
        if unsafe { retype(abi::ObjectType::SmallPage as u64, slot) }.is_err() {
            break;
        }
        if unsafe { map_frame(slot, va + index * 4096) }.is_err() {
            unsafe { drop_frame(slot) };
            break;
        }
        ptr::addr_of_mut!(NEXT_SLOT).write(slot + 1);
        mapped += 1;
    }
    if mapped as usize != length / 4096 {
        // Roll the partial run back: deletion unmaps each frame and makes it
        // collectable, so neither the heap nor the budget records it.
        for index in 0..mapped {
            unsafe { drop_frame(SLOT_BASE + 1 + index) };
        }
        ptr::addr_of_mut!(NEXT_SLOT).write(SLOT_BASE + 1);
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

// ---- Rust global allocator ------------------------------------------------

/// Ready-made `GlobalAlloc` for Rust tasks (interpreter-app.md 决策 B): one
/// allocator implementation shared by the C staticlib and every Rust binary.
/// An application only writes
/// `#[global_allocator] static HEAP: rstiny_alloc::Heap = rstiny_alloc::Heap;`.
///
/// Alignments above the 16-byte block granularity report failure (null), the
/// `GlobalAlloc` contract's "return null" outcome; no shipped task allocates
/// over-aligned types.
pub struct Heap;

unsafe impl core::alloc::GlobalAlloc for Heap {
    unsafe fn alloc(&self, layout: core::alloc::Layout) -> *mut u8 {
        if layout.align() > 16 {
            return core::ptr::null_mut();
        }
        unsafe { alloc(layout.size()) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, _layout: core::alloc::Layout) {
        unsafe { dealloc(pointer) }
    }

    unsafe fn realloc(
        &self,
        pointer: *mut u8,
        layout: core::alloc::Layout,
        new_size: usize,
    ) -> *mut u8 {
        if new_size == 0 {
            unsafe { dealloc(pointer) };
            return core::ptr::null_mut();
        }
        if layout.align() > 16 {
            return core::ptr::null_mut();
        }
        unsafe { reallocate(pointer, new_size) }
    }
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

/// IPC buffer of the calling thread; the kernel publishes its address in the
/// read-only `tpidrro_el0` register (same convention as the user library).
unsafe fn ipc_buffer() -> *mut abi::IpcBuffer {
    let address: usize;
    // SAFETY: read-only system register read; the kernel owns the mapping.
    unsafe {
        core::arch::asm!(
            "mrs {address}, tpidrro_el0",
            address = out(reg) address,
            options(nomem, nostack)
        );
    }
    address as *mut abi::IpcBuffer
}

/// Fire one `svc #0` Call on `cap` and return the reply word. Words beyond the
/// four register message registers and every capability travel through the
/// task's IPC buffer (`Retype` carries six words plus the destination CNode).
unsafe fn invoke(cap: u64, label: u64, args: &[u64], caps: &[u64]) -> Result<u64, ()> {
    if args.len() > abi::MAX_MESSAGE_WORDS || caps.len() > abi::MAX_EXTRA_CAPS {
        return Err(());
    }
    if args.len() > 4 || !caps.is_empty() {
        let buffer = unsafe { ipc_buffer() };
        if buffer.is_null() {
            return Err(());
        }
        // SAFETY: the task's runtime owns this buffer; no same-address-space
        // thread or signal reentry exists yet.
        unsafe {
            for (index, &word) in args.iter().enumerate().skip(4) {
                core::ptr::addr_of_mut!((*buffer).msg)
                    .cast::<u64>()
                    .add(index)
                    .write_volatile(word);
            }
            for (index, &slot) in caps.iter().enumerate() {
                core::ptr::addr_of_mut!((*buffer).caps_or_badges)
                    .cast::<u64>()
                    .add(index)
                    .write_volatile(slot);
            }
        }
    }
    let mut tag = abi::MessageInfo::new(label, caps.len(), args.len()).word();
    let mut mr0 = args.first().copied().unwrap_or(0);
    let mut mr1 = args.get(1).copied().unwrap_or(0);
    let mut mr2 = args.get(2).copied().unwrap_or(0);
    let mut mr3 = args.get(3).copied().unwrap_or(0);
    // SAFETY: seL4-style Call; register assignments are the platform ABI.
    unsafe {
        core::arch::asm!(
            "svc #0",
            in("x7") abi::Syscall::Call as i64 as u64,
            inlateout("x0") cap => _,
            inlateout("x1") tag,
            inlateout("x2") mr0,
            inlateout("x3") mr1,
            inlateout("x4") mr2,
            inlateout("x5") mr3,
        );
    }
    if abi::MessageInfo::from_word(tag).label() == abi::OK {
        Ok(mr0)
    } else {
        let _ = (mr1, mr2, mr3);
        Err(())
    }
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
