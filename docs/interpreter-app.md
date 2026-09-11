# 以 MicroPython 为驱动样例的解释器应用与架构完善

微内核的用户态需求往往被"hello world"式样例掩盖。本文用 **MicroPython**——一个
真正的"大 C 应用"(解释器:大段、长寿命堆、REPL 双向 I/O、可选文件系统、可崩溃
可重启)——作为驱动样例,反推并完善 RSTiny 的用户态架构。本文只做设计结论与
决策,不承诺具体移植;实施与否另走 `docs/evolution-plan.md` 的阶段表。

参考资料:
- 入口/调用 ABI:[sel4-abi.md](sel4-abi.md)
- 服务协议与 loader 监督:[service-manager.md](service-manager.md)
- 磁盘栈与 fs 协议:[disk-driver.md](disk-driver.md)
- 演进规划与验收约定:[evolution-plan.md](evolution-plan.md)

## 1. 目的与范围

1. 用解释器暴露"普通 hello 应用"不触发的问题:大 `.bss`、需要 `free` 的堆、
   长文件名/多 client 的文件系统、C 运行时接入、崩溃恢复。
2. 给出**决策**(编号 A–G),区分"架构必须完善"与"移植细节"。移植本身(第三
   方 C 代码)不在本文范围。
3. 与 seL4 的做法逐条对照,保留已有"原生 Python 在微内核里不是主流"的边界
   (Python 在 seL4 生态里是宿主工具 + Linux guest,见 §10)。

## 2. 现状盘点(代码事实)

| 维度 | 现状 | 位置 |
| --- | --- | --- |
| EL0 入口 | 普通任务 `_start(x0 = SpawnInfo 页面 VA, SP = loader 给)`;root 走 `__rstiny_root_start` | `rstiny-runtime-macros`、`projects/libs/user/src/elf.rs` |
| loader | 逐 4 KiB 页:retype → copy alias → map → 拷贝字节 → unmap → delete → map,约 7 次内核调用/页;段含 `.bss`(零填充也逐个映射) | `elf.rs` `spawn`/`spawn_supervised` |
| 栈 | 子进程栈 64 KiB | `elf.rs` |
| 用户态堆 | 无通用库;`mysh` 自备 512 KiB **bump** 分配器(`global_allocator`) | `projects/apps/mysh/src/main.rs` |
| console | `WRITE`(≤112 字节/次)、`READ`(轮询 RX,非阻塞) | [service-manager.md](service-manager.md) §14.2 |
| fs | **单个绑定 client**、名字 ≤ 13 字节(8.3 短名)、句柄 4、共享一页、`READDIR` 分页 | [disk-driver.md](disk-driver.md) §8.2 |
| 服务协议 | READY/REPORT/EXIT/PING/STOP/DEPENDENCY_LOST + fault、重启策略(never/on-failure/always/backoff) | [service-manager.md](service-manager.md) §9/10/12 |
| 时间 | `Runtime::Clock`/`Sleep` | sel4-abi.md |
| 构建 | Rust-only;`tools/build_app.py` + `make_disk.py`(mtools);C 工具链 `aarch64-linux-gnu-gcc 14.2` 已在宿主机 | Makefile |
| 性能 | debug 下 mysh spawn ≈ 14 s(TCG),其中 loader 页风暴占大头;release ≈ 0.6 s | 实测(2026-09-11) |

## 3. MicroPython 的诉求 vs 现状

| # | 解释器诉求 | 现状差距 | 架构完善(决策) |
| --- | --- | --- | --- |
| 1 | 大段:代码 ~200–500 KB + 可选 512 KB 级堆/栈 | loader 逐页 ~7 次 syscall,debug 下显著慢 | A:loader 批量映射 + 大帧 |
| 2 | 长寿命堆,需要 `free` | 只有 bump 分配器,无回收 | B:用户态通用分配器提升为一等库 |
| 3 | C 入口必须遵守 loader 契约 | 契约只被 Rust 服务隐式消费 | C:显式化 EL0 加载契约 |
| 4 | REPL:读 console + echo + 编辑 | `CONSOLE_READ` 轮询已有 | D:维持轮询,IRQ RX 列为演进 |
| 5 | import 标准库/读写脚本 | fs 单 client、短名、4 句柄 | E:先 frozen stdlib,后 fs v2 |
| 6 | 崩溃可重启、监督 | 服务协议 + 重启策略已覆盖 | — 直接复用 |
| 7 | C 与 Rust 混合构建 | 构建只有 Rust | F:C 产物接入构建/磁盘 |
| 8 | 预算与镜像规模匹配 | `budget` 是 2 的幂、按服务配置 | G:镜像大小 × 页数 ≤ 预算 的验收规则 |
| 9 | 带参运行:`./python app.py` | `SpawnInfo`/`Supervision` 无 argv;`./cmd` 只取命令 token | H:参数页 argv;I:按需授予 fs 能力 |

## 4. 决策 A–G

### 决策 A:loader 批量映射与大帧(内核 + loader 协同)

- 现状:每 4 KiB 帧一次 `Retype`+`Map`,`.bss` 的 128 页(512 KiB 池)也要逐个映射
  → 是 debug 慢的实测主因。
- 目标:`spawn_supervised` 按"同属性连续段"合并:VSpace 支持 2 MiB 大帧
  (`LargePage`),对齐时一个映射覆盖 512 页;不对齐处退化为小页批处理。
- 边界:`Rights/Attr` 同段才可合并;内核对象表按帧记账,大帧记 512 帧。
- 验收:同镜像 debug spawn 时间下降一个量级;`check_mysh`/`check_fat*` 不回归。

### 决策 B:用户态通用分配器(解决 bump 不可释放)

- 现状:`mysh::Bump` 512 KiB,只进不出;解释器需要 malloc/free。
- 目标:`projects/libs/alloc`(或并入 libs/user)提供 freelist + 合并的
  `malloc/realloc/free`,内存来自任务自己的 budget untyped(复用现有
  `retype`/`map` 记账),不新增内核机制。
- 边界:单核、无锁;与 seL4 一致(seL4 用户态自行管理 heap,内核只管 Untyped)。
- 与 C 的桥:`malloc` 符号由 C rt0 重导出到 libc-alloc 或 MicroPython 直接
  用 `mpconfig.h` 指向它。

### 决策 C:显式化 EL0 加载契约

- 现状:`_start(x0 = SpawnInfo VA, SP = loader 提供)`,Rust 宏隐式生成;C 程序
  需要与之一致(可忽略 x0,但不得假设 x0=0)。
- 目标:在 [sel4-abi.md](sel4-abi.md) 增加"加载契约"一节:入口寄存器、栈顶、
  x0 语义、`.bss` 清零保证(loader 现从清零帧 retype,天然满足)、
  W^X(代码段不可写)、64 KiB 栈。
- C rt0 模板:`_start: 忽略 x0 → 调用 `port_main(void)`→ 永不返回。

### 决策 D:console 维持轮询 RX,IRQ RX 列为演进

- 解释器 REPL 在轮询模型下可用(mysh 已示教:空读 `sleep` 重试)。
- 演进:`CONSOLE_READ` 阻塞化需要 PL011 IRQ + Notification(现有 IRQ 授权链
  可用,见 [irq.md](irq.md));不阻塞架构决策,列入阶段表。

### 决策 E:文件系统——先 frozen,后 fs v2

- 阶段 1(不可避免的改动最小):标准库与脚本 **frozen 进解释器镜像**(MicroPython
  的 "frozen modules"),不碰 fs;磁盘上只有 `python` 一个 ELF。
- 阶段 2(架构完善):fs v2 —— 多绑定 client(或按 badge 的多缓冲)、长名(≥255,
  走 IPC buffer 而非 MR 打包)、目录遍历增强。理由:解释器的 import/脚本读写
  按当前 8.3+13 字节 + 单 client 无法表达;这是本次唯一"协议级"扩展点。
- 与 seL4 对照:seL4 生态没有内置 fs;文件系统是 CAmkES 组件或 Linux guest
  内的事。RSTiny 已有 fs 服务,做 v2 属于自有路径,不算偏差。

### 决策 F:C 产物接入构建与磁盘

- `tools/build_app.py` 增加"C 交叉编译"模式:`aarch64-linux-gnu-gcc -nostdlib
  -fno-stack-protector -Wl,-Ttext=0x200000 …`,产出符合 loader 段契约的 ELF。
- `make disk` 允许 `--file python=...` 与 `--file app.py=...`;具体移植时把
  MicroPython 的构建(CMake)收敛成这一步,固定工具链版本(gcc 14.2,与现有
  系统同源)。
- 验收:一个最小 C ELF(`minic`)经 `./minic` 在 mysh 里跑通,再谈解释器。

### 决策 G:预算与镜像规模的验收规则

- 规则:`budget ≥ max(段总长, 镜像 filesz) 上取 2 的幂,且 ≥ 栈+堆期望`。
  解释器估算:release ~300–500 KB 镜像 + 512 KB 堆 + 64 KB 栈 ⇒ `budget = 2M`
  是合理起步;frozen stdlib 可能翻倍,写进 `init.cfg` 注释。
- 验收:镜像变化时 `check_*` 的帧数与 `Runtime::AvailableFrames` 断言同步更新
  (现有 `check_restart` 已示范该模式)。

### 决策 H:参数页 argv(loader 扩展)

- 现状:`SpawnInfo`(x0 指向)无 argv;`Supervision` 无 argv;`./cmd arg`
  目前只取命令 token。
- 设计:参数页仍是 x0 指向的同一页,布局 v2 = `SpawnInfo`(保持不变)+
  紧跟的 `ArgvBlock`:`magic = 0x41525641_00000002`、`argc: u64`、
  `total: u64`、随后 argc 个 NUL 结尾字符串;页内剩余空间容纳(默认
  上限 ~1 KiB,超限抛错)。
- loader:`spawn_supervised` 的 `Supervision` 增加 `argv` 字段并把字符串
  拷进参数页;既有调用(hello/init/console/mysh)不传 argv 则页内无
  `ArgvBlock`,行为与今天完全一致(向后兼容)。
- EL0 契约:rt0 读 `SpawnInfo` 后检查页内 offset 处的 magic;有则组装
  `char *argv[]` 交给 `port_main(argc, argv)`;无则 `argc = 1, argv = [name]`
  或空(依 rt0 约定)。
- mysh:`execute` 把 `./cmd` 之后的 token 全部传入 `run_program`,经 loader
  写入参数页;`./python app.py` ⇒ argv = [`python`, `app.py`]。

### 决策 I:依赖能力授予(解释器按需拿 fs)

- 现状:`run_program` 的 caps 固定给 control/console/budget/ASID;hello
  不需要 fs。解释器要读 `app.py` 却需要 fs client 能力。
- 设计:监督者按需把自身 `fs_ep` **复制**给子进程,固定槽 53(与 init 的
  `CHILD_DEP_BASE` 惯例一致),并在 `SpawnInfo.extra` 对应槽登记;EL0 契约
  声明:"脚本运行类子进程在槽 53 可收到 fs_ep(可能不存在)"。
- 边界:仍是按需最小权限 —— 无脚本参数的 `./hello` 不获得 fs;能力复制
  只给一份,badge 沿用 fs 的 0 或监督者自身 badge。
- C 侧:`port` 需要一个极小的 fs client(OPEN/READ/CLOSE 走既有 fs 协议
  IPC),属于移植工作量,不改协议。

## 5. 与现有文档的联动

| 文档 | 联动 |
| --- | --- |
| sel4-abi.md | 新增"EL0 加载契约"(决策 C) |
| service-manager.md | §3.5/§15 补"解释器亦可作 init 服务(restart=on-failure)" |
| disk-driver.md | fs v2 需求登记(决策 E) |
| evolution-plan.md | 新增阶段 P0–P4(见 §8) |

## 6. 解释器如何落入系统(统一视角)

- **作为 init 服务**(`restart=on-failure`):开机即有 REPL;崩溃由 init 重启;
  console/future fs 依赖走 `depends`。适合"系统自带 Python"。
- **作为 mysh 子进程**(`./python`):与 hello 同走 `spawn_supervised`,但多两个
  机制,支持两种运行方式:
  - `./python`(无参数):解释器内 REPL,直接读写 console(决策 D 的轮询 RX)。
  - `./python app.py`:mysh 把命令后的 token 作为 **argv** 传给子进程(决策
    H:参数页 `ArgvBlock`),并**按需授予一份 fs 能力**(决策 I,槽 53),
    解释器从磁盘打开并解释 `app.py`。
  - 两种都只复用现有 `Service` 协议(READY/EXIT/fault)与 `spawn_supervised`。
- 两种落地方式里,argv 与 fs 授予是仅有的两处架构增量(决策 H/I),其余全部
  复用现有协议;这是"解释器=普通应用"设计最重要的一句话。

## 7. 与 seL4/其他微内核对照

| | seL4 | 本项目(MicroPython 案例) |
| --- | --- | --- |
| 原生 Python | 无维护端口;宿主工具是 Python(CAmkES/sel4test/配置);目标侧跑 Python 走 Linux guest(`libsel4vmm`) | 原生 MicroPython as 服务/应用;无 guest OS 依赖 |
| 用户态堆 | 各 rootserver 自管(heap 自 Untyped 切) | 补 libs/alloc 通用分配器(决策 B) |
| 文件系统 | 无内置;CAmkES 组件 | 已有 fs 服务;fs v2(决策 E) |
| 大应用加载 | ELF loader(elfloader)按页,同样有批量映射空间 | loader 批量映射 + 大帧(决策 A) |
| REPL 串口 | 通常经串口抽象层 | console 服务 READ/WRITE/RX 轮询(决策 D) |

偏差思想:seL4 因为"微内核最小化"不提供原生 Python,是为隔离/认证服务的
取舍;本项目的定位是教学/演示型微内核,把解释器当一等用户应用反而体现
"架构完备性",两者都是正当选择,文档记录差异即可。

## 8. 分阶段实施(待定,进 evolution-plan)

| 阶段 | 内容 | 前置 | 验收 |
| --- | --- | --- | --- |
| P0 | EL0 加载契约文档化(决策 C/H/I);fs v2 与分配器接口定义 | — | 无代码,文档评审 |
| P1 | `libs/alloc` 通用分配器(决策 B);loader 批量映射/大帧(决策 A) | P0 | `check_mysh` debug spawn 时间下降;可用帧断言不变 |
| P2 | C 工具链接入 + `minic` ELF(决策 F);参数页 argv(决策 H) | P1 | `./minic` 在 mysh 跑通;`./minic arg` 读到 argv |
| P3 | MicroPython port:frozen stdlib、堆、console、time、argv 解析(决策 B/D/E-1/H);脚本运行类子进程的 fs 授予(决策 I) | P2 | `./python` 出 REPL;`./python app.py` 解释执行并断言输出 |
| P4 | fs v2(决策 E-2) | P2 | 长名 import 与脚本读写;`check_fs2` |

## 9. 结论

不是移植方案,而是架构完善清单:

1. 必须(使解释器成为可能):决策 B(通用分配器)、F(C 构建接入)、C(契约文档)。
2. 值得(性能/规模):决策 A(loader 批量映射)、G(预算规则)。
3. 演进(非阻塞):决策 D(RX IRQ)、E(fs v2)。
4. 带参运行(`./python app.py`):决策 H(argv)+ I(fs 授予);REPL 模式(`./python`)
   不需要它们。
5. 复用即得的:服务协议、监督/重启、console、时间、budget 模型。

用 MicroPython 换来的主要收益是把上述四点从"设计假设"变成"被一个真实程序
压测过的结论"。