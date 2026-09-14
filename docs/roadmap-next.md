# 下一批工作（2026-09）：P0–P3 优先级规划

把"当前优先做什么"写成可执行清单。它与 [evolution-plan.md](evolution-plan.md) 的
长阶段表互补：本文讲**顺序与理由**（地基 → 机制 → 用户价值 → 体验），并给出每项的
现状事实、设计要点与验收。状态：规划，未承诺实施日。

排序列：**P0 正确性（必做、先做）→ P1 机制最小化收尾 → P2 MicroPython 完成态 →
P3 性能（可选）**。

## P0 IPC 语义正确性（地基，先做）

### P0.1 缺陷①：`assemble` 长消息越界 panic

- 现状（代码事实）：`kernel/src/api/ipc.rs::assemble` 对 `info.length() > 4` 时
  切 `bytes[..(len-4)*8]`（`bytes` 仅 960 字节）并索引 `words[4..len]`（120 词）。
  `len ∈ 121..=127` 时越界 → **内核 panic**。对象调用路径
  `kernel/src/api/message.rs:41` 有 `length() > MAX_MESSAGE_WORDS` 校验，IPC
  路径遗漏。
- 修法：在 `assemble` 入口加与对象路径一致的校验（超限返回 `TruncatedMessage`，
  不 panic、不半交付）；统一收口到同一个长度常量/检查函数，防止两路径再次漂移。
- 验收：`check_ipc` 新增用例——发送 127 词长消息，断言内核不 panic 且发送方
  收到显式错误；全矩阵（debug/release × LOG=off/info × managed/production）。

### P0.2 缺陷②：`Call` 无 `GrantReply` 永久挂起

- 现状：`ipc.rs` 把 `caller` 按 `call && grant_reply` 决定是否入队为
  `Caller::Call`；`call && !grant_reply` 时调用者不入回复队列 → 永久 `BlockedReply`。
- 语义决策（二选一，写进 sel4-abi）：
  - **A（对标 seL4）**：`Call` 要求 endpoint cap 具备 Grant（回复能力可形成），
    否则**立即返回错误**，绝不停留；
  - B：放宽为"无 grant 也可 Call，回复无 badge"（弱化）。
  推荐 **A**：现在就是无权利挂起的错误行为，A 只是把它变成显式错误。
- 验收：`check_ipc` 新增"无 GrantReply 的 Call 立即报错"用例（现状会挂死，
  用例必须先于修复失败）。

## P1 机制最小化收尾（优先级调度 + 睡眠语义）

### P1.1 优先级调度（evolution-plan 阶段 F）

- 现状：单核 FIFO + 10ms 时间片，无优先级（service-manager §17）；
  `TCBSetPriority` 未实现。
- 设计：`Task`/调度器增加优先级字段与就绪多队列，同优先级 round-robin；
  `TcbSetPriority` 纳入 seL4 对象方法（label 7）。策略仍在内核（机制），但
  优先级数值由监督者设置，权限经 TCB cap 校验。
- 验收：新调度用例（高优先级先跑、同优先级轮转）+ 既有 `check_tasks/check_restart`
  不回归。

### P1.2 睡眠的裁决

- 现状：`Runtime::Sleep` 仍是保留的受限原语（内核 `Disposition::Sleep`）；
  timer PPI 被内核自留（`irq.rs`："timer PPI ... stay kernel-owned"）。
- 方案：
  - **A（seL4 式，stretch）**：把 Generic Timer PPI/或独立 timer IRQ 授权给一个
    用户态 timer 服务（走现有 IRQControl/IRQHandler 链），sleep = 向该服务 IPC；
    内核只保留调度 tick。**风险**：单核上内核调度 tick 与用户 timer 争用同一
    PPI，需要内核拥有一个 tick + 用户拿到另一个（QEMU virt PPIs 有限），实现与
    验证成本不低。
  - **B（务实）**：保留 `Sleep` 为"自指、cap 校验的受限原语"（记入
    sel4-abi 与文档，作为对 seL4 的明示偏离——seL4 没有 sleep 系统调用）。
  推荐 **先 B、A 列为 stretch**：与 `Exit/Shutdown` 同类，属于可解释的内核
  便利，不破坏 C0–C3 的能力模型。

### P1.3 C3 最后一米：`Object::Runtime` 物理删除

- C3 已把托管方法门控；若 P1.2 走 B，`Runtime` 实际只剩信息类
  （`Clock/Current/AvailableFrames`）+ 受限原语（`Sleep/Exit/Shutdown`）。
- 可选项：把 `Clock/AvailableFrames` 移入"只读、无副作用"的能力查询
  （或保留 Runtime 仅这两类），随后删除 `Object::Runtime` 与对应 cap 复制。
- 验收：grep userland 无 Runtime 引用；`check_tasks` 等 managed harness 转产
  或标注废弃。

## P2 MicroPython 完成态（用户价值线）

### P2.1 fs v2（P4 硬前置）

- 现状：单绑定 client（`bound: Option<badge>`）、名字 ≤ 13 字节（MR 打包，客户端
  ≤ 12）、`MAX_FILES=4`、单共享页（disk-driver §8.2）。
- 设计：长名（>13，经 IPC buffer 传 ≤ 960 字节，依赖 P0 的长消息正确性）、
  多 client（绑定表 + 每 client 句柄/或共享页复用 + 缓冲分槽）、可选目录路径。
  `hadris-fat` 的 `entry.name()` 已能给出长名，主要是协议与 server 状态扩展；
  标签保持协议分段约定（新增 label 或 v2 后缀），旧客户端向后兼容。
- 验收：`check_fs2`——长名 open/read、两绑定 client 并发、帧断言不回归。

### P2.2 MicroPython 浮点

- 现状：内核 FP/SIMD 惰性上下文已在（fpu.md，528B/任务）；port 仍是整数模式
  （`mpconfigport.h` `FLOAT = 0`、`build_app.py -mgeneral-regs-only`）。
- 设计：开启 `MICROPY_PY_BUILTINS_FLOAT`（选 double/soft），port 构建放行 FP 指令
  前审计 `-O2` 向量化（NEON 现在会被 FPU 上下文接住，不再 trap）；mpy-cross
  与脚本语义同步。
- 验收：`check_python` 加浮点断言（`1.5*2.0`、`math` 模块）；整数模式切换为
  配置项仍可回退。

### P2.3 `sys.argv[0]` 约定钉死

- 现状：`./cmd arg…` 的 argv 直接是命令后 token（`argv[0]=第一个参数`），与
  mysh 注释"程序名在 [0]"不一致（C2 遗留）。
- 决策：要么 shell prepend 程序名（`argv=[cmd, arg…]`），要么 port 按
  "argv[0]=脚本路径"处理（MicroPython 常见语义）。二选一后同步
  `micropython-port.md` 与 `sel4-abi.md` 加载契约。
- 验收：`./python app.py` 时 `sys.argv` 符合约定；无参时 REPL。

### P2.4 集成

- `check_python` **挂进 `make check`**（当前未挂）；P2 全矩阵验收。

## P3 loader 批量映射 + 大帧（体验/性能，可选）

- 现状：逐 4KiB 页装载（~7 次 syscall/页）；`ObjectType` 无 `LargePage`；debug
  启动慢（在 C0 之前实测 mysh spawn ~14s，release 0.6s；C0 之后 BSS 大头已消）。
- 设计：新增 `LargePage`（2MiB 块）对象 + VSpace L2 块映射；loader 对同属性
  连续段（尤其零填充 BSS）在 2MiB 对齐时用块映射，否则退化为小页；记账按
  512 帧/块。影响面：object/mod.rs 记账、memory/plan_map、elf.rs、check_untyped。
- 验收：debug 启动时间对比（目标明显下降）；帧/watermark 断言不回归。

## 依赖与顺序

| 项 | 依赖 | 排他 |
| --- | --- | --- |
| P0 | 无 | 全前置（长消息正确性是 P2.1 的前提） |
| P1.1 优先级 | 无（调度器内部） | 可并行 |
| P1.2 睡眠 B | 无 | 可并行 |
| P2.1 fs v2 | P0（长消息/cap 传递） | P2 内部先行 |
| P2.2 float | fpu.md（已就绪） | 可并行 |
| P3 | 内核对象层 | 独立，纯性能 |

建议主线：**P0 → (P1.1 ∥ P2.1) → P2.2/2.3 → P3**。P1.2 裁决与 P1.3 可随
P1.1 一并收尾。

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