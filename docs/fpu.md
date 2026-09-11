# FP/SIMD 上下文与惰性切换（开启 FPU 支持）

日期：2026-09-15。状态：已实施（F0 内核改造与 F1 验收测试已落地，`make check` 全绿）。本文把 [完整微内核设计与分阶段路线](microkernel-design.md) 与 [能力系统与 IPC 演进规划](evolution-plan.md) 中列为"明确不做"的 FP/SIMD 上下文拆成独立里程碑，给出开启 EL0 FPU 支持的设计与验收。§14 列出的既有文档已按 F3 回写。

## 1. 现状与缺口

- `kernel/src/arch/kernel/thread/user.rs` 的 `configure_el0_domain()` 置 `CPACR_EL1 = 0`：EL0 与 EL1 的 FP/SIMD 访问全部陷入。`UserContext`/`TrapFrame`（272 字节）不含任何 FP 寄存器状态。
- `tools/check_tasks.py` 已实证该行为：任务执行 FP 指令 `0x9e670000`（`fmov`）被以 `ESR_EL1.EC = 0x07`（FP/SIMD Access Trap）作为用户故障终止。
- 现有用户程序按 `aarch64-unknown-none-softfloat` 编译，运行库与 Rust 代码不产生 FP 指令，因此"所有用户 FP 都故障"今天不咬人；但 C 载荷（未来 loader）、MicroPython 浮点构建和手工 FP 测试程序都无法运行。
- 内核自身也是 softfloat 编译，从不使用 FP/SIMD；`KernelContext` 切换、entry trap 路径均与 FP 无关。
- 兄弟内核 `/root/codes/x-kernel` 在 AArch64 上只提供一次性 `enable_fp()`（置 `CPACR_EL1.FPEN = TrapNothing`），x86_64 的 `ctx.rs` 则是每次切换全量保存 FPU 状态——本项目取 seL4 的惰性方案（见 §4/§5）。

## 2. 目标与非目标

目标：

- EL0 任务可透明使用基础 FP/SIMD（AArch64 `V0..V31`、`FPCR_EL1`、`FPSR_EL1`），无需 ABI 变更或用户侧改动。
- 惰性上下文切换：只有真正使用 FP 的任务付出保存/恢复代价；不用 FP 的任务零额外陷入。
- 跨任务隔离：任何任务的 FP 状态不得泄漏给其他任务；新任务首次使用 FP 时看到清零初始状态。
- 全部改造保持在 EL0/EL1 边界内单核串行完成，复用一个 `SingleCore` 封装的全局状态，不加锁。
- 回归全绿：`make check`、`LOG=off`、内核自身从不意外执行 FP 指令。

非目标（明确裁剪）：

- **不做 SVE**：平台为 QEMU virt `-cpu cortex-a72`，无 `FEAT_SVE`；`CPACR_EL1.ZEN` 保持复位禁止。SVE 指令（若被实现）走 `EC 0x19`，未实现平台上为未定义指令故障，两类都被现有用户故障路径终止隔离。
- **不做 FPCR 异常陷入**：任务初始 `FPCR = 0`，全部异常不陷入、只置 `FPSR` 标志。若用户自行写 FPCR 使能异常陷入，对应同步异常不属于 `EC 0x07`，仍走普通用户故障路径（视为不支持配置，终止任务）。
- **不做 AArch32**、不做 fp16 相关新特性、不做每核状态（SMP 后再议，见 §13）。
- **不给内核开放 FP**：`CPACR_EL1` 的默认态保持"EL1 也陷入"，作为内核误用 FP 的即时故障哨兵。

## 3. AArch64 硬件事实（设计必须遵守）

| 事实 | 值/语义 |
| --- | --- |
| FP 状态 | `V0..V31` 各 128 位（512 字节）+ `FPCR_EL1`（控制，复位 0）+ `FPSR_EL1`（状态，复位 0），合计 528 字节/任务 |
| `CPACR_EL1.FPEN`（bits 21:20） | `0b00`：EL0 与 EL1 的 FP/SIMD 均陷入（本内核现状）；`0b11`：均放行；另有 `0b01`（只陷 EL0，EL1 放行）可作备选 |
| `CPACR_EL1.TFP`（bit 30） | 同时陷 EL0 与 EL1 的 FP/SIMD（含 SVE），**无法只隔离 EL0**；本设计不使用它，SMP 惰性切换可另行评估 |
| `CPACR_EL1.ZEN`（bits 17:16） | SVE 门控；无 `FEAT_SVE` 时保留 |
| 陷入异常类 | `ESR_EL1.EC = 0x07`（FP/SIMD Access Trap），同步、EL0 发起、`ELR_EL1` 指向故障指令本身 |
| 关键推论 | **当 EL1 也被陷入时（现状 `CPACR_EL1 = 0`），保存/恢复用的 `stp q0,q1,..` 自身也会陷入**——迁移函数必须先开 FP 再执行寄存器搬运 |

寄存器封装：`aarch64-cpu` crate 提供了 `CPACR_EL1` 类型化封装（`FPEN::TrapNothing`），但没有 `FPCR_EL1`/`FPSR_EL1` 封装；V 寄存器只能从内联/裸汇编使用。保存与恢复需要 `stp q0..q31`（16 对）与 `mrs/msr fpcr,fpsr`。

### 3.1 两个 CPACR 状态

本设计只使用两个全局值，与现状（0）和兄弟内核 `enable_fp()` 的取值一致：

| 常量 | 值 | 效果 |
| --- | --- | --- |
| `CPACR_DISABLE` | `0`（现状） | EL0、EL1 的 FP/SIMD 全陷入 |
| `CPACR_ENABLE` | `0b11<<20`（`0x00300000`） | EL0、EL1 全放行 |

非所有者任务进入用户态前写 `CPACR_DISABLE`；所有者任务进入用户态前写 `CPACR_ENABLE`。除 `fpu.rs` 的迁移窗口外，内核任何时刻都保持 `CPACR_DISABLE`，维持"内核误用 FP 立即以 EC 0x07 致命 panic"的现状哨兵。

## 4. seL4 惰性切换模型

seL4（`fpu.c`）在单核 AArch32/AArch64 上维护一个 **FPU 所有者**（最近使用 FP 的线程），规则是：

1. 线程切出时（或被换入前），若换入者不是所有者，使 FP 陷入。
2. 任何人（非所有者）执行第一条 FP 指令 → `EC 0x07` 陷入 → 内核把**前任所有者**的 528 字节保存进它自己的上下文，换入当前线程的状态，更新所有者，关闭陷入，重放指令。
3. 用不着 FP 的线程完全不付账；所有者任务反复进出 CPU 也不产生 FP 陷入。

关键性质：**硬件寄存器里的 FP 状态属于"最近的 FP 用户"**，无论该用户当前是否在跑；该用户被换出后状态仍留在寄存器里，直到另一个用户到来。因此被换出的所有者只需保存一次——在它被替代的那个瞬间。

本项目映射：所有者身份 = 持有稳定 `FpuContext` 盒的任务（含代数的完整任务 ID），配合 `SingleCore` 全局状态实现上面的规则。seL4 允许内核执行期间拥有 FP 状态者不是当前线程；本项目相反，让**内核运行时一律保持 `CPACR_DISABLE`**，只在迁移窗口短暂放行。

## 5. 设计取舍：全量保存 vs 惰性

| 维度 | A：全量保存（x-kernel `ctx.rs` 风格） | B：惰性切换（seL4 风格，本文采用） |
| --- | --- | --- |
| 路径 | 每次 EL0→EL1 陷入都随 TrapFrame 存 528+ 字节；syscall/IRQ/故障全付账 | 只在一个任务首次（或所有权变更后首次）用 FP 时付账 |
| 不动 FP 的任务 | 每次系统调用都白付 | 完全零开销 |
| 定时器抢占（10 ms） | 每次抢占付账 | 所有者不变则零开销 |
| 实现复杂度 | 低（改 `TrapFrame` + trap.S 保存对） | 中（新增所有者状态机 + 迁移路径） |
| 与 TrapFrame 272 字节布局断言 | 要动（成大结构，每陷入多拷 544 字节） | 不动（`TrapFrame` 断言原样保留） |
| 确定性延迟 | 好（无迁移陷入） | 首个 FP 指令多一次陷入（微秒级，可接受） |
| 与 seL4 教学主线 | 偏离 | 契合（文档多处与 seL4 对齐） |

选 B。代价集中在新增的 `fpu.rs` 与一处运行循环改动，收益是现状回归（`check_tasks.py` 大量软浮点任务）完全不受影响。A 方案（在 `TrapFrame` 里加 544 字节并按陷入次数搬运）作为退路在 §12 记录，不实施。

## 6. 数据结构与所有权

```rust
// kernel/src/arch/kernel/thread/fpu.rs（新模块）
/// 每任务 FP 现场。放在 UserContext 的堆盒内，地址与 Execution 生命周期一致且稳定。
#[repr(C)]
#[derive(Debug, Default, Clone, Copy)]
pub(crate) struct FpuContext {
    q: [u128; 32],      // V0..V31，512 字节
    fpcr: u64,          // FPCR_EL1（控制寄存器快照）
    fpsr: u64,          // FPSR_EL1（状态寄存器快照）
}
const _: () = assert!(size_of::<FpuContext>() == 528);

struct Owner {
    id: u64,                  // 完整任务 ID（含代数），defensive 校验
    ctx: *mut FpuContext,     // 稳定盒子地址，仅迁移窗口解引用
}

/// 单 CPU 全局，与 SCHEDULER/GIC 一样经 SingleCore 访问，要求 IRQ 已屏蔽。
struct Fpu {
    owner: Option<Owner>,
}

fn migrate(cur: &mut FpuContext);   // §7.2
fn activate(cur: &mut FpuContext);  // §7.1，每次 eret 前调用
fn forget(id: u64);                 // 任务销毁/退出/故障终止时调用
```

要点：

- `FpuContext` 作为 `UserContext` 的新字段放进既有堆盒（`Execution.frame: NonNull<UserContext>`），**不新增分配**；地址跨任务槽↔调度器移动保持稳定，满足 §4 的"被换出的所有者仍需按地址可寻址"。
- `Owner` 存 `*mut FpuContext` 裸指针。迁移时前任必非当前运行者，不可能与任何进行中的 `&mut` 借用重叠（单核、IRQ 屏蔽）；`id` 含代数用于防御性校验 `forget` 遗漏。
- `owner == Some(self)` 的判定比较 `id` 即可，不比较裸指针。
- 内核的 `TrapFrame` **不增加 FP 字段**，272/112/24 字节布局断言与 trap.S 的 SAVE_REGS/return_user 全部不动。

## 7. 执行流

### 7.1 放行/陷入决策：`activate()`，每次 eret 前

`UserContext::run()` 目前每次进入前设置 `TPIDRRO_EL0`。在其旁加：

```rust
fpu::activate(self);   // self: &mut UserContext（字段 fpu）
// owner 是当前任务 → CPACR_EL1 = ENABLE；否则 CPACR_EL1 = DISABLE
```

每次 eret 都执行一次 `msr`，值为常量或现状值，开销可忽略；这保证所有进入用户的路径（首次运行、抢占恢复、park 恢复、FP 迁移后的重入）语义一致，不依赖调度器的上下文细节。

### 7.2 迁移：`migrate()`，EC 0x07 分支

在 `run()` 的循环里把 `EC 0x07` 从"用户故障"拆出来：

```rust
pub unsafe fn run(&mut self, root: usize, ipc_buffer: usize) -> UserEvent {
    assert!(instructions::irq_masked());
    loop {
        let mut trap = RawTrap::default();
        memory::activate(root);
        fpu::activate(self);                       // §7.1
        TPIDRRO_EL0.set(ipc_buffer as u64);
        unsafe { run_user(&mut self.0, &mut trap) };
        memory::activate_kernel();
        match trap.kind {
            1 => return UserEvent::Interrupt,
            0 if trap.esr >> 26 == 0x15 && trap.esr & 0xffff == 0 => return UserEvent::Syscall,
            0 if trap.esr >> 26 == 0x07 => fpu::migrate(self),   // ← 新增
            0 => return UserEvent::Fault(/* 不变 */),
            _ => unreachable!(...),
        }
    }
}
```

`migrate` 的固定顺序（IRQ 已屏蔽，trap.S 原样带出 ESR/FAR，未触碰 `TrapFrame`）：

```text
1. 断言 owner 为空或 owner.id 是合法任务（防御性；正常路径由 forget 保证）。
2. CPACR_EL1 ← ENABLE        # 先开 FP：此后的 stp q/mrs fpcr 才不陷入
3. 若 owner 非空：把硬件 q0..q31、fpcr、fpsr 存入 *owner.ctx
4. 把当前任务 fpu 的 q0..q31、fpcr、fpsr 载入硬件（新任务为零值）
5. owner ← 当前任务；状态保持 ENABLE（本次 eret 前 activate 会再写一遍同一值）
6. return 到 run() 循环 → 再次 run_user → ELR 未动，故障指令原样重放
```

第 2 步之后、第 6 步重入用户之前，是内核中唯一允许执行 EL1 FP 指令的窗口：窗口内不允许调用任何可能使用 FP 的代码（`log::*`、分配器等），模块边界以 rustdoc 固化该不变量；softfloat 目标只保证 rustc 不生成 FP 指令，不保证未来手写内联汇编也遵守，故窗口要尽量小、且集中在 `fpu.rs`。

### 7.3 为什么重放是安全的

- FP 访问陷阱是同步指令级故障，`ELR_EL1` 指向故障指令，没有部分完成的可观察副作用；重放入口与普通 eret 完全同构（同一 `TrapFrame` 未修改、同一栈、同一地址空间）。
- 迁移在 EL1 完成，期间无 EL0 执行、IRQ 屏蔽、单核，不存在"其他任务同时改 FP 状态"的竞争。
- 迁移本身不改 GPR/ELR/SPSR，`run_user` 的载荷原样继续。

## 8. 任务生命周期集成

| 事件 | 动作 | 位置 |
| --- | --- | --- |
| 创建/Start | `FpuContext` 随 `UserContext` 盒分配并**显式清零**（`FPCR=FPSR=0`，IEEE 默认、无异常陷入） | `task/runtime.rs`、`task/execution.rs` 构造处 |
| 首次 eret | `activate`：`owner` 非当前 → DISABLE → 首条 FP 指令陷入 → `migrate` 载入零状态 | `user.rs` |
| 任务被换出/换入 | 无需动作；下次 eret 的 `activate` 决定陷入与否，硬件状态本就属于最近所有者 | — |
| 任务 Exit | 调度器 retire 路径调 `fpu::forget(id)`：若 `owner.id == id` 则 `owner = None`（终止任务不再需要保留状态；硬件陈旧状态在下次 migrate 被覆盖） | `task/scheduler.rs` retire/destroy |
| 任务 Fault 终止 | 同上，经同一 destroy/回收路径 | `task/scheduler.rs` |
| 任务被 Destroy | 同上（含运行中任务销毁非所有者等情况；`forget` 对非所有者无操作） | `task/api.rs` → scheduler |

不变式：**任何使任务槽/Execution 可回收的路径必须先把 owner 从该任务摘掉**。若遗漏，后继 migrate 会把状态写进已释放的 `FpuContext` 盒（堆复用场景的 UAF）。由于 owner 只在 `SingleCore` 全局里，把 `forget` 收敛进调度器的单一 retire/destroy 代码点即可保证；`Owner.id` 的代数校验是第二道防线。

## 9. 隔离与安全

- **不泄漏**：任何两个任务之间不存在共享 FP 窗口（单核 + IRQ 屏蔽 + 迁移原子）；换入任务的首次 FP 陷入必先保存前任、再载入自身。
- **新任务零初始化**：即使分配器已清零，`FpuContext` 也显式零初始化（防止盒复用读旧值），并作为回归断言（§11）验证。
- **非法指令仍然隔离**：非 FP 的原生非法指令（EC 0x00 等）继续走用户故障终止；FPCR 异常陷入、SVE（`EC 0x19`）同样落入现有故障路径，不扩大特权面。
- **内核保持被陷**：默认态 `CPACR_DISABLE` 保留"内核误用 FP → EC 0x07 → fatal panic 就地定位"的现状哨兵；迁移窗口是唯一例外且集中在 `fpu.rs`。
- **无新 ABI**：用户零改动；不引入使能 FP 的系统调用或 capability（seL4 同：FPU 是架构透明资源）。

## 10. 改动清单（逐文件）

| 文件 | 改动 |
| --- | --- |
| `kernel/src/arch/kernel/thread/fpu.rs`（新） | `FpuContext`（528B）、`Fpu`/`Owner`、`SingleCore` 全局、`migrate`/`activate`/`forget`、两个 CPACR 常量、V 寄存器与 FPCR/FPSR 的 asm 保存/恢复助手（`#[target_feature(enable="neon")]`，附软浮点 target 的 `aarch64_softfloat_neon` 说明）、rustdoc 固化"迁移窗口内不得调用用 FP 的代码" |
| `kernel/src/arch/kernel/thread/user.rs` | `UserContext` 改为二元组 `(TrapFrame, FpuContext)`；`run()` 变循环：每次 eret 前 `activate`、`EC 0x07` 分支 `migrate` 后重放、返回任何事件前 `disable`；`configure_el0_domain` 注释更新为两态说明 |
| `kernel/src/task/runtime.rs`、`scheduler.rs` | `run_user_thread_loop` 携带任务 id；`current_root()` 返回 `(id, root, ipc_buffer)`；`finish()` 在释放执行盒/上下文前调 `fpu::forget(id)` |
| `kernel/src/arch/kernel/trap.S`、`context.rs`、`kernel_context.rs` | **不改**（ESR/FAR 原样带出，分类在 Rust；FP 不在 TrapFrame 里） |
| `tools/check_tasks.py` | 原 `0x9e670000 → EC 0x07` 用例替换为 UDF `0x00000000 → EC 0x00`（FP 行为移至 check_fpu.py） |
| `tools/check_fpu.py`（新，接入 `make check`） | 六组场景，见 §11 |
| §14 列出的既有文档 | 已回写（“禁用 FP”表述与里程碑归属均已更新） |

## 11. 测试与验证

- **新 `tools/check_fpu.py`**（release，QEMU cortex-a72，已接入 `make check`）：
  1. 数值正确：`fmov/fmul` 链计算 `(2.0)^4`，`exit` 返回精确 IEEE-754 位型 `0x4030000000000000`；
  2. 新任务全零现场：直接 `mrs` FPSR/FPCR 得到 0（读取本身只在不被陷的窗口内成立）；
  3. 所有权交接 A→B→A：A 置 `d0=5.0` 后 sleep，B 接管并置 `d0=7.0` 退出，A 醒来仍读到自己的 5.0（migrate 往返）；
  4. FPCR/FPSR 持久化：FPCR.FZ 与一次不精确除法置起的 FPSR.IXC 跨交接保留（多数惰性实现会漏这两个系统寄存器）；
  5. 持有者被销毁：FP 持有者 sleep 时被 destroy，新任务读 `d0` 必须为 0（`forget` 防悬垂写）；
  6. 真实非法指令仍隔离：UDF #0（EC 0x00）照常终止任务，证明 FP 是透明放行而非吞掉所有异常。
  结束后校验 `available` 与 `mappings` 无泄漏。
- **回归修订**：`check_tasks.py` 原 `0x9e670000 → EC 0x07` 用例在 FP 开启后不再故障；改为 `0x00000000`（UDF，EC 0x00），`0x24`（数据中止）与 `0x18`（PC 对齐）用例不变。
- **全量**：`make check` 绿（含 check_bootloader/check_kernel/check_userboot/check_fault_handler/check_capabilities/check_untyped/check_ipc/check_tasks/check_fpu/check_user_context/check_relocation/check_block/check_fat32/check_appmgr/check_mysh/check_services）。

## 12. 实施顺序与验收

| 步骤 | 交付 | 退出条件 |
| --- | --- | --- |
| F0 | `fpu.rs` 骨架：`FpuContext`、两态常量、asm 保存/恢复助手、`run()` 的 EC 0x07 分支与 `migrate` 占位（暂只重入不迁移） | 现状回归全绿（hello/fatboot 均软浮点，无行为变化） |
| F1 | 完整 `migrate`/`activate`/`forget` + 生命周期接线（创建清零、retire/destroy forget） | `check_fpu.py` 用例 1–5 过 |
| F2 | `check_tasks.py` 用例替换 + 惰性负担下限验证 | 全量 `make check` 绿 |
| F3 | 文档回写（§14） | 无陈旧的“禁用 FP”表述 |
| F4 | 可选试点：MicroPython 浮点构建 / 性能基线（FP 任务与纯软浮点任务混跑的陷入计数） | 试点验收文档化 |

**实施状态**：F0–F3 已完成（`make check` 56 项全绿，含 check_fpu.py 六组场景；文档已回写）。F4 保留为可选后续。

退路（不实施，仅记录）：方案 A 在 `TrapFrame` 尾部加 528 字节并按陷入全量搬运——改动集中但每次 syscall/IRQ/故障都付账；若未来出现"几乎所有任务都用 FP + syscall 密集"的负载，可回退比较。

## 13. 风险与裁剪

| 风险 | 缓解 |
| --- | --- |
| 内核在迁移窗口意外执行 FP 指令（软浮点 target 只约束 rustc） | 窗口集中 `fpu.rs` 并 rustdoc 固化不变量；默认态仍 `CPACR_DISABLE`，误用即当场 fatal panic 定位 |
| 悬垂 owner（forget 遗漏） | forget 收敛到调度器单一路径 + `Owner.id` 含代数防御；`KERNEL_TEST` 覆盖销毁所有权任务 |
| FP 指令重放被误解为故障投递 | EC 0x07 在 `run()` 内联处理，永不进入 `faults.rs`；重放无副作用（§7.3） |
| SVE/fp16/未来扩展 | 基线裁剪（§2）；新指令按未定义/`EC 0x19` 故障隔离，不扩大面 |
| SMP 到来时状态机失效 | 单核先行：owners 每核一份、`CPACR` 每核写；届时评估 TFP/中断化惰性切换；本文不引入跨核共享 |
| 惰性导致的一次性延迟抖动 | 单次陷入微秒级；验收不设延迟硬指标，只要求负担下限不退化（§11） |

## 14. 对既有文档的影响（实施后回写）

- `docs/user-execution.md` §三种上下文："FP/SIMD 通过 CPACR_EL1 禁止"改为"惰性放行，`UserContext` 附带 528 字节 FP 现场"。
- `docs/micropython-port.md` §7：浮点不再是内核级硬门槛，改为"可选启用浮点的前置条件是 FP/SIMD 上下文（已实现）"。
- `docs/memory-task.md` §验证："实际执行 EL0 FP 指令验证权限陷入"改为"验证 FP 指令被透明放行、未定义/SVE 指令仍隔离"。
- `docs/evolution-plan.md` §11：从"明确不做"清单移除"FP/SIMD 上下文"，或在里程碑表中标注已独立成里程碑。
- `docs/microkernel-design.md`：补一句 FP/SIMD 里程碑指向本文。
- `README.md`："不支持 … FP/SIMD"表述更新，文档列表追加本文链接。