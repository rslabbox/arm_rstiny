# MicroPython 移植实现设计(落地 `interpreter-app.md` 的阶段 P2–P3)

把 [interpreter-app.md](interpreter-app.md) 的架构决策落到一份**可以开工的实现
设计**:在 RSTiny 上跑 Mini-less 的 MicroPython,做到

- `./python` → 解释器内 REPL(读写 console);
- `./python app.py` → 通过参数页 argv(决策 H)+ 槽 53 的 fs 能力(决策 I)从
  磁盘读取脚本并解释执行;
- 整数模式:端口保留整数 MicroPython(内核已支持 FP,见 §7;此模式不启用浮点)。

本文是"实现设计",不是架构(架构见 interpreter-app.md),也不包含第三方
C 代码正文(移植时从 MicroPython 上游拉取)。

## 0. 落地状态（2026-09，P3 已完成）

- P1 `libs/alloc`（决策 B）✅、P2 C 交叉编译 minic + argv（决策 F/H）✅；
- P3（本 port）✅：`third_party/micropython`（tag v1.24.1 子模块级固定）+ `ports/micropython-rstiny`，
  `make disk` 带 `python.elf` 与 `APP.PY`；`./python` REPL（`print(1+2)`→`3`，
  Ctrl-D 退出）与 `./python app`（fs 槽 53 + ArgvBlock，`APP.PY` 脚本）均以
  `tools/check_python.py` 验收（debug/release × LOG=off/info）；
- 已知微瑕：个别内建异常类型名（如 ZeroDivisionError）的 qstr 打印为空
  （帧与参数在，类型名缺，MINIMUM qstr 表边界）；不影响 REPL/脚本验收。
- 未做：决策 A 批量映射优化（debug 启动仍逐页）、fs v2、frozen modules。

## 1. 目标与非目标

| 目标 | 验收 |
| --- | --- |
| `./python` 出 `>>>` REPL,`print(1+2)` → `3` | `check_python.py` |
| `./python app.py` 解释磁盘脚本并输出 | `check_python.py` |
| 解释器退出 → 经 control_ep EXIT 被 mysh 回收 | `[mysh] ./python exited: 0` |
| 不显著改变既有行为(debug/release × LOG=off/info 回归)` | 现有 `make check` |

非目标(明说,避免过度设计):

- 不做浮点(`MICROPY_PY_BUILTINS_FLOAT = 0`),理由与可选启用条件见 §7。
- 不做线程/`_thread`、socket、timezone;`MICROPY_ROM_LEVEL` 从 `MINIMUM` 起步。
- 不做 fs v2:脚本用现有 fs 协议(8.3 短名、`APP.PY` 合法)整体读入内存执行;
  标准库 import 来自 **frozen modules**(决策 E-1)。fs v2 留给 P4。
- 不在内核加任何机制(USER 页 UV 已支持;全靠决策 A/B/H/I 的用户态/loader 增量)。

## 2. 前置依赖(先做,均已有设计)

| # | 内容 | 出处 | 说明 |
| --- | --- | --- | --- |
| 1 | `libs/alloc` 通用分配器,导出 C 符号 `malloc/realloc/free` | 决策 B | 给 `m_malloc` 用 |
| 2 | 参数页 argv(`Supervision.args` + `ArgvBlock`) | 决策 H | P2 先做,`minic` 验证 |
| 3 | 槽 53 fs 能力按需授予 | 决策 I | `./python app.py` 时授予 |
| 4 | C 交叉编译接入 | 决策 F | `aarch64-linux-gnu-gcc 14.2` 已有 |

## 3. 仓库与工具链

- MicroPython 上游以 **子模块** 固定 tag(如 `v1.24.x`),放 `third_party/micropython`。
- 我们的 port 放 `ports/micropython-rstiny/`(仓库内,不污染上游):
  实现 `mpconfigport.h`、`mphalport.h/c`、`rt0.S`、`main.c`、`fs_client.c`、
  `linker.ld`、`Makefile`、`frozen/`。
- 编译:`aarch64-linux-gnu-gcc -nostdlib -nostartfiles -ffreestanding
  -fno-stack-protector -fno-unwind-tables` 编 `cpy/py/*.c + cpy/extmod/selected +
  port/*.c`,链接 `linker.ld`(入口 `_start`,段基 0x200000,与 Rust 应用同契约)。
- 产物 `python.elf` 经 `make disk --file python=...` 进磁盘(8.3 短名 `PYTHON`)。

## 4. 内存与预算布局

| 区域 | 大小 | 来源 |
| --- | --- | --- |
| 代码 + rodata(整数模式) | ~250–400 KB(release,`MINIMUM` 更小) | ELF 段 0x200000 起 |
| GC 堆(Python 对象) | 256 KB | `.bss` 静态数组,`gc_init` |
| C 堆(`m_malloc`) | 128 KB | `libs/alloc` 在 budget 内切 |
| 栈 | 64 KB | loader 提供(必要时提 128 KB) |
| 参数页 | 4 KB(含 argv 块 ≤1 KB) | loader 建 |

合计 ~ <1 MB ⇒ `budget = 2M` 起步(interpreter-app.md 决策 G)。debug 下 loader
按页映射 ~240 页,慢是已知问题(决策 A 缓解,不阻塞本移植)。

对齐/声明:`ALIGN(16) static char gc_heap[MP_GC_HEAPSIZE];` + `gc_init`。

## 5. port 文件与 MicroPython 钩子

| MicroPython 钩子 | RSTiny 实现 |
| --- | --- |
| `mp_hal_stdout_tx_strn(s, n)` | console `WRITE`,分块(每块 ≤ 112 字节,协议 `MAX_WRITE`) |
| `mp_hal_stdin_rx_chr()` | console `READ`(轮询)+ 空读 `sleep(5ms)` 重试(与 mysh 同法) |
| `mp_hal_ticks_ms()` / `mp_hal_delay_ms()` | `Runtime::Clock` / `Runtime::Sleep` |
| `malloc/realloc/free`(C 符号) | `libs/alloc`(决策 B) |
| `gc_init` | `.bss` 数组 |
| Ctrl-C | 轮询得到的 0x03 直接返回;REPL 行内取消 |
| `MICROPY_PY_SYS_PLATFORM` | `"rstiny"` |

REPL 本体:`pyexec_friendly_repl()`(`py/pyexec.c`)只依赖上述 stdout/stdin 两个
钩子,port 无需自写行编辑。

## 6. 启动序列

```
_start(x0 = 参数页 VA)            ; loader 已设 SP=栈顶,段/DSS 已就绪
  -> rt0: 解析 SpawnInfo + 可选 ArgvBlock(决策 H)
  -> libs/alloc 初始化(在 budget 内切 C 堆)
  -> gc_init(gc_heap)
  -> mp_init()
  -> argc > 1 ? 执行脚本 : pyexec_friendly_repl()
  -> mp_deinit()
  -> call(control_ep, EXIT, code) ; 由 mysh 回收;不发 Runtime::Shutdown
```

- REPL 里 Ctrl-D 结束解释器 → 走上面 EXIT,回到 mysh 提示符;整机关机仍是
  mysh 的 `exit`(Runtime::Shutdown)。
- 崩溃:解释器 fault → 经 control_ep 投给 mysh,`[mysh] ./python spawn failed
  / exited` 按既有路径处理;做 init 服务(可选项)时 `restart = on-failure`。

## 7. 整数模式（可选浮点）

EL0 浮点已在内核侧落地：`UserContext` 附带 528 字节 FP 现场，CPACR_EL1 按任务
惰性放行，首次执行 FP 指令时陷入保存/恢复（见 [FP/SIMD 上下文与惰性切换](fpu.md)）。
本端口仍保持 `MICROPY_PY_BUILTINS_FLOAT = 0`（整数 Python）：

- 微观层面 `MICROPY_FLOAT_IMPL` 相关宏不启用；
- `mpconfigport.h` 里显式关闭 float；如需启用浮点，前置条件（FP/SIMD 上下文）
  已实现，再为 MicroPython 选择 float 实现并接入即可，不再是内核级硬门槛；
- 测试脚本只能用整数语义（`1+2`，字节串等），验收断言里全是整数输出。

## 8. 脚本执行(`./python app.py`)

1. mysh:`execute("python app.py")` 解析 token → `run_program("python", args=["app.py"])`;
   `Supervision.args` 非空 ⇒ loader 写 `ArgvBlock`(决策 H),并在槽 53 复制
   `fs_ep` 给子进程(决策 I)。
2. rt0 组装 `argv = ["python","app.py"]` → `port_main(2, argv)`。
3. `port_main` 用 **fs client**(槽 53,`OPEN("APP.PY")→READ→CLOSE`)把整个脚本
   读进 C 堆缓冲区(≤ 64 KB,超出报 `OverflowError` 语义)。
4. 内存中执行:`mp_parse(src, MP_PARSE_FILE_INPUT)` → `mp_compile` →
   `mp_call_function_0`(即 `exec` 语义);或先用 mpy-cross 预编译 `.mpy`,
   `pyexec_frozen_module` 直跑字节码(须前者先通)。
5. `./python`(无 args):不授予 fs,不走脚本路径,直接 REPL。

fs client 只需 `OPEN/READ/CLOSE/BIND`,几十行 C,不改协议(名字 `APP.PY` 合法
8.3 短名)。标准库与常跑脚本用 frozen modules(`frozen/`),避免 import 触盘。

## 9. mysh / loader 代码增量(H/I 的具体改动)

- `projects/libs/user/src/elf.rs`:
  - `Supervision` 增 `args: &[&str]`;
  - 加载时若 `args` 非空,在参数页 `SpawnInfo` 之后写
    `ArgvBlock{magic, argc, total, strings…}`(页内剩余空间,超限返回错误);
- `projects/apps/mysh/src/main.rs`:
  - `execute`:`./cmd arg…` 把剩余 token 收进 `Vec`,传入 `run_program`;
  - `run_program(stem, args)`:`args` 非空时追加
    `ChildCap{ slot: 53, source: fs_ep, rights, badge: 0 }`,并在
    `SpawnInfo.extra[dep] = 53`(槽位与 EL0 契约一致,见
    [sel4-abi.md](sel4-abi.md) 加载契约);
- 参数页 v2 与 argv 解析的 C 侧在 rt0(`ports/micropython-rstiny/rt0.S/c`)。
- REPL 路径:`args` 为空 → 不授予 fs,行为与今天 hello 相同。

## 10. 构建与磁盘

- `tools/build_app.py` 增 `--lang c`(或独立 `tools/build_python.py`):
  调 port 的 Makefile → `python.elf` → 校验段/入口(复用现有 ELF 校验)。
- Makefile:`disk:` 增加 `--file python=$(APP_DIR)/python.elf`;init.cfg(可选):
  ```text
  service python { elf = "python.elf"; depends = fs; restart = on-failure; budget = 2M }
  ```
  (默认**不**加,保持缺省拓扑干净;解释器主要经 mysh `./python` 使用。)
- `check_python.py`:debug/release × LOG=off/info,断言:
  1. `./python` 出 `>>>`;输入 `print(1+2)` 断言 `3`;输入 `exit()`(或 Ctrl-D×2)
     后 `[mysh] ./python exited: 0`;QEMU 仍由 mysh `exit` 关机;
  2. 磁盘放 `APP.PY`(内容如 `print("hi from disk")`),`./python app.py` 断言
     输出,再 `exit` 回到 mysh;
  3. 全程 `kernel panic` 不出现;与 `check_mysh` 幂等(不共享串口状态)。

## 11. 风险与裁剪

| 风险 | 缓解 |
| --- | --- |
| debug 下 loader 映射 ~240 页慢(实测 mysh 已 ~14 s) | 决策 A(批量映射)先做;release 0.6 s 无感 |
| 64 KB 栈对 REPL 递归紧 | 提至 128 KB;`MP_STACK_CHECK` 打开 |
| 整数模式限制表达能力 | 目标明确:REPL + 简单脚本;文档§7 说明 |
| GC 堆 256 KB 对列表操作偏小 | 常量可调;脚本验收只用小数据 |
| 子模块体积/构建时长 | 固定 tag;首次构建后台化;CI 缓存 |
| `./python app.py` 依赖 fs_client | 先 REPL 后脚本;fs_client 独立小文件可单测 |

## 12. 实施顺序(对齐 evolution-plan P0–P3)

1. P0:本设计评审(契约、边界);
2. P1:`libs/alloc` + loader 批量映射(决策 A/B);
3. P2:C 工具链接入 + `minic` + argv(决策 F/H)→ minic 验收;
4. P3:MicroPython port(§5–§8)+ `check_python.py`;先 REPL,再 `./python app.py`;
5. (P4,可选)fs v2:长名 import / 触盘标准库。

## 13. 对照与偏差

- seL4 生态:目标机无原生 Python(宿主工具 + Linux guest);本设计走"原生
  MicroPython as 普通应用",偏差已登记在 [interpreter-app.md](interpreter-app.md) §7。
- MicroPython 惯例:内置 port 用 RTT/posix/硬件驱动;本 port 用 RSTiny 的
  服务协议(console/fs)+ 参数页契约,是自有路径,port 内 README 说明。