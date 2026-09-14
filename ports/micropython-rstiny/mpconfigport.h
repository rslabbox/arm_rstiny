/*
 * MicroPython port for ARM RSTiny (interpreter-app.md 决策 E/P3).
 *
 * Float configuration (P2.2): the kernel keeps a lazy per-task FPU/SIMD
 * context (docs/fpu.md, 528 B/task), so EL0 may execute FP/NEON instructions
 * and traps only to save/restore state. Double precision matches the
 * target's soft-float ABI (FP values in integer registers across calls) and
 * needs no extra ABI work; MICROPY_PY_MATH comes with the vendored
 * lib/libm_dbl implementations (see the port Makefile). Building with
 * `FP=0` reverts to the integer-only configuration (no -mgeneral-regs-only
 * escape from -Os vectorisation needed since the FPU context exists).
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

// Float mode (P2.2): double precision on the kernel's lazy FPU context.
// `FP=0` in the port Makefile flips these back to the integer-only build.
#if MICROPY_RSTINY_FLOAT
#define MICROPY_FLOAT_IMPL         (MICROPY_FLOAT_IMPL_DOUBLE)
#define MICROPY_PY_BUILTINS_FLOAT  (1)
#define MICROPY_PY_MATH            (1)
#else
#define MICROPY_FLOAT_IMPL         (MICROPY_FLOAT_IMPL_NONE)
#define MICROPY_PY_BUILTINS_FLOAT  (0)
#endif
#define MICROPY_PY_BUILTINS_COMPLEX (0)

// Full error text: ROM_LEVEL MINIMUM would default to TERSE reporting, whose
// compressed error strings ("~..." ROM-text encoding) have no compression
// table in this freestanding port, silently dropping messages like
// "ZeroDivisionError: division by zero".
#define MICROPY_ERROR_REPORTING (MICROPY_ERROR_REPORTING_NORMAL)

// GC heap: a 256 KiB .bss array (docs: micropython-port.md §4). C-heap growth
// of our allocator is separate (C heap via rstiny-alloc, retyped from the
// task's own Untyped budget).
#define MICROPY_HEAP_SIZE (256 * 1024)

#define MICROPY_ALLOC_PARSE_CHUNK_INIT (16)
#define MICROPY_ALLOC_PATH_MAX (256)

// sys module: gives the REPL `exit()` (SystemExit) and sys.platform.
// sys.argv carries the shell tokens verbatim (P2.3 convention): argv[0] is
// the script path as given to `./python`, followed by the remaining tokens.
#define MICROPY_PY_SYS (1)
#define MICROPY_PY_SYS_ARGV (1)
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