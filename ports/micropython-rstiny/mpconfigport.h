/*
 * MicroPython port for ARM RSTiny (interpreter-app.md 决策 E/P3).
 *
 * Integer-only configuration: the kernel puts EL0 in a domain with no FPU
 * context (CPACR_EL1 = 0), so any FP/SIMD instruction traps immediately.
 * MICROPY_FLOAT_IMPL_NONE therefore stays the default and float support is
 * deliberately left off. Enabling it would require kernel-side FP context
 * first, not a config change here.
 *
 * The port is freestanding (no libc): malloc/realloc/free come from the
 * rstiny-alloc staticlib, string helpers from shared/libc/string0.c, and
 * setjmp/longjmp from this port's own setjmp.h/setjmp.c built on GCC's
 * __builtin_setjmp (py/nlr.c falls back to setjmp on AArch64).
 */

#ifndef MICROPY_INCLUDED_PORTS_RSTINY_MPCONFIGPORT_H
#define MICROPY_INCLUDED_PORTS_RSTINY_MPCONFIGPORT_H

#include <stdint.h>
#include <stddef.h>

// Start from the most minimal feature set: compiler, REPL helper, GC.
#define MICROPY_CONFIG_ROM_LEVEL (MICROPY_CONFIG_ROM_LEVEL_MINIMUM)

#define MICROPY_ENABLE_COMPILER (1)
#define MICROPY_HELPER_REPL     (1)
#define MICROPY_ENABLE_GC       (1)

// Python source is read from the fs client into RAM and compiled there.
#define MICROPY_ENABLE_EXTERNAL_IMPORT (0)

// Integer mode (see header comment).
#define MICROPY_FLOAT_IMPL         (MICROPY_FLOAT_IMPL_NONE)
#define MICROPY_PY_BUILTINS_FLOAT  (0)
#define MICROPY_PY_BUILTINS_COMPLEX (0)

// Full error text: ROM_LEVEL MINIMUM would default to TERSE reporting, whose
// compressed error strings ("~..." ROM-text encoding) have no compression
// table in this freestanding port, silently dropping messages like
// "ZeroDivisionError: division by zero".
#define MICROPY_ERROR_REPORTING (MICROPY_ERROR_REPORTING_NORMAL)

// GC heap: a 256 KiB .bss array (docs: micropython-port.md §4). Heap growth
// of our allocator is separate (C heap via rstiny-alloc, Runtime::Map).
#define MICROPY_HEAP_SIZE (256 * 1024)

#define MICROPY_ALLOC_PARSE_CHUNK_INIT (16)
#define MICROPY_ALLOC_PATH_MAX (256)

// sys module: gives the REPL `exit()` (SystemExit) and sys.platform.
#define MICROPY_PY_SYS (1)
#define MICROPY_PY_SYS_PLATFORM "rstiny"
#define MICROPY_HW_BOARD_NAME "rstiny"
#define MICROPY_HW_MCU_NAME "cortex-a72"

// Freestanding: no <alloca.h>; GCC provides the builtin.
#define alloca __builtin_alloca

// Machine word types must be pointer-sized.
typedef intptr_t mp_int_t;
typedef uintptr_t mp_uint_t;
typedef long mp_off_t;

// Port state lives in the VM state (no thread state beyond it).
#define MP_STATE_PORT MP_STATE_VM

#endif // MICROPY_INCLUDED_PORTS_RSTINY_MPCONFIGPORT_H