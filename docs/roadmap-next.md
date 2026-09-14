# 下一批工作（2026-09）：P0–P3 优先级规划

把"当前优先做什么"写成可执行清单。它与 [evolution-plan.md](evolution-plan.md) 的
长阶段表互补：本文讲**顺序与理由**（地基 → 机制 → 用户价值 → 体验），并给出每项的
现状事实、设计要点与验收。

状态（2026-09 实施记录）：**P0、P1.1、P1.2、P2.1–P2.4 已实施**；P1.3 裁决为
**暂缓**（理由见该节）；P3 保持**可选未排期**。

排序列：**P0 正确性（必做、先做）→ P1 机制最小化收尾 → P2 MicroPython 完成态 →
P3 性能（可选）**。

## P0 IPC 语义正确性（地基，先做）——已实施 2026-09

### P0.1 缺陷①：`assemble` 长消息越界 panic ——已实施

- 落地：`MessageInfo::valid()`（abi crate，length ≤ 120、capsUnwrapped == 0）
  成为两条路径共用的唯一校验：对象调用路径与 `ipc.rs::assemble` 入口均调用；
  另在 IPC syscall 边界（Send/NBSend/Call/Reply/ReplyRecv）预检 caller 的
  出站 tag——非法 tag 立即报 `TruncatedMessage` 给发送方自身，不会进入等待
  queue 污染其他 receiver。
- 验收：`check_ipc.py` 新增 127 词 CALL 用例（修复前内核 panic、GDB 失联；
  修复后发送方得到 `(7<<12)|1` 且对端无投递），全矩阵 4 组合通过。
  用例先于修复验证过失败。

### P0.2 缺陷②：`Call` 无 `GrantReply` 永久挂起 ——已实施（方案 A）

- 落地：`ipc.rs` 在 endpoint 权限检查后追加 `Call && !RIGHTS_GRANT_REPLY →
  PERMISSION_DENIED`，不投递、不入队、绝不驻留；语义记入
  [sel4-abi.md](sel4-abi.md)（"Call 要求 GrantReply"）。
- 验收：`check_ipc.py` 新增 WRITE-only cap 上 CALL 的用例（修复前调用方永久
  驻留、harness 超时；修复后立即得 `(3<<12)|1`），先于修复验证过失败。

## P1 机制最小化收尾（优先级调度 + 睡眠语义）

### P1.1 优先级调度（evolution-plan 阶段 2）——已实施 2026-09

- 落地：`Task.priority: u8`（默认 0，未显式设置时与原 FIFO 行为完全一致）；
  就绪队列 `RunQueue::pop_best` 每次出队取最高优先级、同级保持入队 FIFO
  （抢占/让出回队尾 = 同级轮转）；优先级在出队时重估，对任何状态的任务即时
  生效。`TcbSetPriority`（label 7）只有一个 `priority` 字（0..=255，
  越界 `RangeError`），被调 TCB cap 即授权（WRITE 在 dispatch 校验），不做
  seL4 的独立 authority cap / maxPriority——记入 sel4-abi.md 明示简化。
- 验收：`check_tasks.py` 新增两组用例——高优先级（badge 调度窗口内跑完才轮到
  同级 peer，peer 全程沉默）与 FIFO 对照组（早绑定者先完成）；另有 SetPriority
  负例（>255、非 TCB cap、只读 TCB cap）。既有用例全部不回归。

### P1.2 睡眠的裁决——已裁决 2026-09（方案 B）

- 裁决：**方案 B**。`Runtime::Sleep` 保留为受限自指原语（只驻留调用者、
  Runtime cap 校验、无跨任务/资源副作用），作为对 seL4 的明示偏离记入
  [sel4-abi.md](sel4-abi.md)；方案 A（用户态 timer 服务）降为 stretch——
  单核上调度 tick 与用户 timer 争用 PPI 的拆分成本与当前收益不匹配。
  [evolution-plan.md](evolution-plan.md) 阶段表同步。

### P1.3 C3 最后一米：`Object::Runtime` 物理删除——暂缓

- 裁决：P1.2 走 B 之后，`Runtime` 是受限原语（`Sleep/Exit/Shutdown`、监督类
  `Unmap/Protect/Write/Read`）+ 信息类的载体；物理删除它需要先给这些原语
  重新找家（例如 TCB 自指方法），是一次纯粹的 ABI 翻新，没有行为收益。
  托管面已由 C3 从生产镜像编译掉，"最小化"目标已达成。**暂缓**，待下次
  ABI 演化窗口（如 P3 的对象层改动）一并评估。

## P2 MicroPython 完成态（用户价值线）——已实施 2026-09

### P2.1 fs v2——已实施

- 落地：协议版本 2（`BIND` 按请求版本向下兼容回应，MicroPython C 端 v1 客户端
  零改动）；绑定表按 **endpoint badge** 区分并发 client（`MAX_CLIENTS=4`），
  每个 client 一张私有句柄表（bump 堆上，`lfn` 后 `FileEntry` ≈580B 不宜放栈）
  + 一页私有共享缓冲（server retype 4 页）；长名（≤255 字节）经 IPC buffer
  打包进 MR，`OPEN`/`STAT` 共用；`hadris-fat` 开 `lfn` 特性按长名匹配。
  badge 语义：badge 0 保留一个匿名槽（= v1 单 client 世界，appmgr 等 unbadged
  依赖授予继续工作，第二个匿名绑定被拒）；并发 client 各自 mint badge——
  mysh 自身 badge 1、子进程统一 badge 2（loader 从未加 badge 的依赖 cap
  mint）。READDIR 仍列 8.3 短名。
- 验收：新增 `tools/check_fs2.py`（挂入 `make check`）——长名文件 open/read
  内容校验、mysh+minic 两个并发绑定（LOG=info 断言 `bound as #0/#1`）、第二个
  client 绑定/读取/退出后第一个 client 仍被正确服务。

### P2.2 MicroPython 浮点——已实施

- 落地：`MICROPY_FLOAT_IMPL_DOUBLE` + `MICROPY_PY_BUILTINS_FLOAT` +
  `MICROPY_PY_MATH`（vendored `lib/libm_dbl`，经端口 `libm_shim/math.h` 编译，
  绕开 glibc `<math.h>` 与 musl 派生内部符号冲突；`stubs.c` 补 freestanding
  `nan()`）。端口 Makefile 放行 FP/NEON（内核惰性 FPU 现场接住 `-Os` 向量化），
  `make FP=0` 一键回退整数模式。
- 验收：`check_python.py` 新增 REPL `print(1.5*2.0, 7//2)` → `3.0 3`、脚本
  `1.5*2.0` → `3.0`、`math.sqrt`/`abs` → `math ok`；FP=0 构建仍通过。

### P2.3 `sys.argv[0]` 约定钉死——已裁决 2026-09（"argv = shell token 序列"）

- 裁决：**不做 shell prepend**。argv 就是 `./cmd` 之后的 token 序列，
  `argv[0]` 即脚本路径（MicroPython 常见语义；解释器自己知道它是 python），
  与 C 的 `argv[0]=程序名` 惯例的偏离已记入
  [interpreter-app.md](interpreter-app.md) 决策 H 与
  [sel4-abi.md](sel4-abi.md) 加载契约；mysh 的过时注释修正。
- 落地：端口开 `MICROPY_PY_SYS_ARGV`，`port_main` 把 `g_argv` 同步进
  `sys.argv`。
- 验收：`check_python.py` 断言 `./python app` 时脚本内 `sys.argv[0] == 'app'`
  （`argv0=app`）；无参仍进 REPL。

### P2.4 集成——已实施

- `check_python` 与 `check_fs2` 均已挂入 `make check`；本文所有改动以
  全矩阵（debug/release × LOG=off/info，managed 套件按需 `MANAGED=1`）
  通过为准。

## P3 loader 批量映射 + 大帧（体验/性能，可选）

- 现状：逐 4KiB 页装载（~7 次 syscall/页）；`ObjectType` 无 `LargePage`；debug
  启动慢（在 C0 之前实测 mysh spawn ~14s，release 0.6s；C0 之后 BSS 大头已消）。
- 设计：新增 `LargePage`（2MiB 块）对象 + VSpace L2 块映射；loader 对同属性
  连续段（尤其零填充 BSS）在 2MiB 对齐时用块映射，否则退化为小页；记账按
  512 帧/块。影响面：object/mod.rs 记账、memory/plan_map、elf.rs、check_untyped。
- 验收：debug 启动时间对比（目标明显下降）；帧/watermark 断言不回归。

## 依赖与顺序

| 项 | 依赖 | 状态 |
| --- | --- | --- |
| P0 | 无 | ✅ 已实施（2026-09） |
| P1.1 优先级 | 无（调度器内部） | ✅ 已实施（2026-09） |
| P1.2 睡眠 B | 无 | ✅ 已裁决（方案 B） |
| P1.3 Runtime 删除 | P1.2 = B | ⏸ 暂缓（见该节） |
| P2.1 fs v2 | P0（长消息/cap 传递） | ✅ 已实施（2026-09） |
| P2.2 float | fpu.md（已就绪） | ✅ 已实施（2026-09） |
| P2.3 argv | 无 | ✅ 已裁决 + 实施 |
| P2.4 集成 | P2.1–P2.3 | ✅ 已实施（2026-09） |
| P3 | 内核对象层 | 可选，未排期 |

远期方向：图形显示栈（QEMU virt + virtio-gpu，受信 `gpu-server` + 单客户端帧缓冲租约）见 [gui-display.md](gui-display.md)；依赖 P0（长消息）与设备/IRQ 机制，不与 P0–P3 冲突，可在其完成后独立开工。

建议主线：~~P0 → (P1.1 ∥ P2.1) → P2.2/2.3 → P3~~——P0 至 P2.4 已完成，
P1.3 与 P3 为余下的可选项。

## 验收纪律

- 每个 PR 过 `make check` 全矩阵（debug/release × LOG=off/info；managed
  harness 按需 `MANAGED=1`）。
- `check_python` 挂入 `make check`。
- 涉及语义/协议/加载契约的改动同步 sel4-abi.md、micropython-port.md、
  evolution-plan.md（阶段表回填 "2026-09 已实施"）。

引用链：[sel4-philosophy.md](sel4-philosophy.md)（差异）·
[capability-authority-untyped.md](capability-authority-untyped.md)（C0–C3）·
[interpreter-app.md](interpreter-app.md)（决策）· [micropython-port.md](micropython-port.md)（P3 现状）
· [irq.md](irq.md)（IRQ 链）· [fpu.md](fpu.md)（浮点）。