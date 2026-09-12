# RSTiny MicroPython port Makefile (docs/micropython-port.md §3).
#
# Builds `python.elf` with the native py core + this port's glue, fully
# freestanding (no libc): rstiny-alloc adds malloc/realloc/free later via the
# RSTiny build integration (tools/build_app.py --lang python); this Makefile
# produces the standalone ELF the loader validates.
#
# Same cross contract as projects/apps/minic: -mgeneral-regs-only (no FPU at
# EL0, interpreter-app.md), -nostdlib, entry `_start` at 0x200000.

include ../../third_party/micropython/py/mkenv.mk

# Cross-compilation to AArch64 EL0.
CROSS_COMPILE = aarch64-linux-gnu-
CROSS_CFLAGS = -nostdlib -nostartfiles -ffreestanding \
	-fno-stack-protector -fno-unwind-tables -fno-pic \
	-fno-asynchronous-unwind-tables -mno-outline-atomics \
	-mgeneral-regs-only
CFLAGS += $(CROSS_CFLAGS) -Wall -Wextra -std=gnu99

# Debug vs release (mirrors the rest of the tree).
ifeq ($(DEBUG), 1)
CFLAGS += -O0 -g
else
CFLAGS += -Os -DNDEBUG -fdata-sections -ffunction-sections
endif

# qstr definitions (before including py.mk)
QSTR_DEFS = qstrdefsport.h

INC += -I.
INC += -I$(TOP)
INC += -I$(BUILD)
CFLAGS += $(INC)

include $(TOP)/py/py.mk

# Port sources: platform glue, entry, stubs, printf, freestanding libc
# shims, and the REPL runtime (pyexec/readline) exactly as ports/minimal.
SRC_C = \
	main.c \
	rstinyhal.c \
	stubs.c \
	shared/libc/printf.c \
	shared/libc/string0.c \
	shared/readline/readline.c \
	shared/runtime/pyexec.c \
	shared/runtime/stdout_helpers.c \

# No frozen modules for the first cut: the REPL and `./python app.py`
# (fs-source + compile) cover the acceptance.
# SRC_C += $(BUILD)/_frozen_mpy.c

SRC_QSTR += shared/readline/readline.c shared/runtime/pyexec.c

OBJ += $(PY_CORE_O)
OBJ += $(addprefix $(BUILD)/, $(SRC_C:.c=.o))
OBJ += $(BUILD)/rt0.o

# Entry stub: argument page VA comes in x0, .bss is cleared, then C startup.
$(BUILD)/rt0.o: rt0.S
	$(ECHO) "CC $<"
	$(Q)$(CC) $(CFLAGS) -c $< -o $@

all: $(BUILD)/python.elf

# Link against rstiny-alloc (decision B) for malloc/realloc/free; the .a is
# produced by tools/build_app.py before invoking this Makefile.
MODE ?= debug
# rstiny-alloc static lib (interpreter-app.md 决策 B): built by
# tools/build_app.py --lang python into the shared app target dir.
ALLOC_A ?= $(abspath ../../target/apps/$(MODE)/aarch64-unknown-none-softfloat/$(MODE)/librstiny_alloc.a)

$(BUILD)/python.elf: $(OBJ) $(ALLOC_A) linker.ld
	$(ECHO) "LINK $@"
	$(Q)$(CC) $(CFLAGS) -nostdlib -T linker.ld -o $@ $(OBJ) $(ALLOC_A)
	$(Q)$(SIZE) $@

# Convenience: strip a copy for the disk image (release may already ship
# unstripped; the Makefile disk target uses `strip` on it).
strip: $(BUILD)/python.elf
	$(Q)$(CROSS_COMPILE)strip --strip-all $(BUILD)/python.elf

include $(TOP)/py/mkrules.mk