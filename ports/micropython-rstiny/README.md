# MicroPython RSTiny port

MicroPython for ARM RSTiny (docs/micropython-port.md, interpreter-app.md 决策 E).

## Layout

- This directory (`ports/micropython-rstiny/`) is the port: it compiles the
  upstream py core into a freestanding EL0 application (`python.elf`).
- The upstream MicroPython source is **not committed** (it is large). Fetch it
  with a fixed tag first:

  ```sh
  mkdir -p third_party
  git clone --depth 1 --branch v1.24.1 https://github.com/micropython/micropython.git third_party/micropython
  ```

  Then build the interpreter:

  ```sh
  make disk MODE=debug      # or release; builds python.elf into the disk image
  ```

  (`make disk` runs `tools/build_app.py python --lang python`, which invokes
  this port's Makefile with `MODE` and links the rstiny-alloc staticlib.)

## Port contents

- `mpconfigport.h` — integer mode (no FPU at EL0), GC heap, error reporting.
- `mphalport.h` / `rstinyhal.c` — console WRITE/READ, Clock/Sleep, the fs
  client (BIND + frame map + OPEN/READ/CLOSE, 决策 I), stack-scan `gc_collect`.
- `main.c` — REPL (`pyexec_friendly_repl`) and `./python app.py` script path.
- `rt0.S` / `linker.ld` — EL0 entry, .bss clear, shared loader contract.
- `Makefile` — py.mk-based build against `third_party/micropython`.

Acceptance: `tools/check_python.py` (REPL + `./python app` across
debug/release × LOG=off/info).