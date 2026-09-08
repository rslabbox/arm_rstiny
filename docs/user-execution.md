# 用户态执行控制权反转设计

状态：已实现。本文记录当前单核内核的用户态执行与返回协议。

## 目标与决策

将“异常入口直接分发并永不返回”改为“运行用户上下文，返回陷入事件，由 Rust 调度循环处理”。ELF 装载、root 启动布局和用户入口宏仍属于各自层次，控制权反转不改变它们的职责。

参考 x-kernel 的 `arch/kcpu/src/aarch64/userspace.rs::UserContext::run`、`arch/kcpu/src/aarch64/excp.S` 和 `posix/process/src/runtime.rs::run_user_thread_loop`。其关键机制是保存进入用户态前的内核调用现场，使 lower-EL trap 最终通过 `ret` 返回 `run()`，而不是在异常处理路径上直接跳入下一任务。

本项目采用共享内核栈上的单一执行循环，不引入每任务独立内核栈。每个用户任务仍拥有独立地址空间与用户上下文；内核在处理完事件并释放局部资源后，才运行下一任务。sleep、wait 和将来的阻塞 IPC 通过显式等待状态实现，不把挂起的 Rust 调用栈作为任务状态。

这与 x-kernel 的完整实现不同：x-kernel 将每个用户线程的运行循环承载在可调度的内核任务中。可返回的用户执行接口本身并不要求这种一对一内核任务模型。只有需要在深层内核调用中挂起、稍后从同一调用点继续时，才值得引入持久内核上下文及每任务内核栈。

约束：单 CPU、EL0 抢占、EL1 不抢占；继续使用当前 FIFO 和 10ms 定时器。暂不增加 SMP、异步 Rust 执行器、内核线程、TLS、FP/SIMD 保存或 POSIX 信号。

## 执行模型

旧的重置 SP、`enter() -> !`、`dispatch() -> !` 及不返回的用户 trap handler 已删除。所有正常用户陷入最终回到调用 `UserContext::run()` 的 Rust 执行循环；内核致命异常仍不返回。

runtime 在一轮结束时完成事件提交，再选择下一任务。用户上下文在 Task 的 `Option<UserContext>` 与私有 ActiveRun 之间转移，运行期间不会持有调度器借用。idle 使用同一条内核调用链，既不重置栈也不打开内核 IRQ。

## 模块边界

| 模块 | 职责 |
| --- | --- |
| `arch/context.rs` | 纯寄存器布局与 Rust/汇编偏移断言 |
| `arch/user.rs` | 私有构造的 UserContext、可返回的执行边界、陷入原因解码 |
| `arch/trap.S` | 保存/恢复用户及内核现场，区分 EL0 返回路径和 EL1 异常路径 |
| `arch/trap.rs` | EL1 故障诊断、不可恢复的架构事件；不选择用户任务 |
| `arch/irq.rs` | GIC acknowledge、设备源处理、EOI，报告是否发生定时器事件 |
| `task/runtime.rs` | 唯一长期执行循环，组织选择、运行、提交事件和 idle |
| `task/scheduler.rs` | 任务表、运行队列、等待关系、状态转换与回收 |
| `task/syscall.rs` | 解码和验证调用，执行对象操作，返回明确的完成/等待/退出结果 |
| `task/boot.rs` | 建立初始任务，然后调用 runtime 的运行入口 |

runtime 使用本内核的具体 syscall handler 即可，不为了模仿 x-kernel 而先增加可插拔 trait 或回调注册表。发生实际多运行环境需求时，再提取接口。

## 三种上下文分别归谁所有

### UserContext：每个用户任务的持久状态

包含 x0..x30、SP_EL0、ELR_EL1、SPSR_EL1，底层复用现有 TrapFrame 布局。字段通过初始化、系统调用结果写回等受控接口修改，用户输入不能直接构造任意 SPSR 或内核返回地址。

用户 ABI 继续使用 softfloat，TLS 当前不支持；以后启用这些功能时，必须同时扩展保存/恢复范围。

### ActiveRun：执行循环暂时拥有的运行状态

调度器在 IRQ 关闭且短期借用有效时选出任务，将其 UserContext 移出任务槽，连同带 generation 的 TaskId 和页表根组成 ActiveRun。任务槽明确标记 Running，context 字段为 None，禁止第二次取出。其 AddressSpace 仍由任务槽持有。

离开调度器借用后才能进入 EL0。此时单核内核不执行其他调度策略，EL0 trap 只保存现场并返回，不能销毁任务或释放地址空间，因此页表根在整次 run 中稳定。提交事件时验证 TaskId，再归还上下文并变更状态。不能同时保留可写的槽内上下文和一份活动副本。

ActiveRun 是私有的一次性对象，不向普通调用者暴露可脱离任务生命周期使用的页表根。这样保留现有全局调度器也不会把 `SingleCore::Borrow` 带入用户执行阶段。

### KernelReturnFrame：一次 run 调用的临时状态

该记录为 112 字节。放在可信 EL1 栈上，保存 AAPCS64 所要求的 x19..x30、恢复内核调用链所需的信息，以及用户上下文和陷入结果缓冲区的可信指针。该记录只存在于一次调用期间，始终 16 字节对齐，不加入用户 TrapFrame，也不映射给 EL0。

保留当前 64 KiB 内核栈及保护页，不按任务数复制。汇编与 Rust 使用 `offset_of!`、`size_of!` 和传入汇编的常量核对布局，不能再在多个文件中散布偏移数字。

## 可返回的执行边界

对上层提供的概念接口如下：

```rust
pub(crate) enum UserEvent {
    Syscall,
    Interrupt,
    Fault(UserFault),
}

impl ActiveRun {
    // 私有封装保证空间存活、上下文有效、本 CPU 唯一活动执行。
    fn run(&mut self) -> UserEvent;
}

// 更底层的 UserContext 执行方法只由上述封装调用；如暴露裸页表根，
// 必须标记 unsafe 并写明生命周期、映射和 IRQ 前置条件。
```

`UserFault` 保存原始 ESR、ELR、来源和仅在有效时存在的 FAR。ESR/FAR 在返回其他 Rust 处理逻辑之前抓取，不能等到嵌套异常之后再读取。同步异常中的 SVC #0 解码为 Syscall，其他同步异常保持现有故障策略。SVC 的 ELR 已指向下一指令，不再手动加 4。未知调用号仍是普通 syscall 错误，不是用户故障。

FIQ/SError 不直接归为普通任务错误，尤其异步 SError 不一定能归因于当前用户；继续走内核致命诊断路径。EL1 同步异常也不能伪装成 `run()` 的正常返回。

### AArch64 进入与返回顺序

1. 进入时要求 DAIF.I 已设置，无调度器、帧池或控制器借用存活。
2. 创建临时 KernelReturnFrame，保存内核 callee-saved 寄存器与返回地址。SP_EL1 留在可信内核栈；不再重置为栈顶。
3. 激活该任务的 TTBR0，执行现有屏障和 TLB 维护，恢复用户寄存器，通过 `eret` 进入 EL0t。用户 PSTATE 允许 IRQ。
4. lower-EL 向量在内核栈暂存完整用户现场，保存原因和故障寄存器，并将用户现场写回 ActiveRun 的上下文。必须在借用 x0 等作为临时寄存器之前保存其用户值。
5. 根据可信栈记录恢复内核调用现场，通过 `ret` 回到执行包装层。此时仍在 EL1，IRQ 关闭。
6. 包装层立即切回内核空 TTBR0，完成屏障/TLB 维护，然后向 runtime 返回 UserEvent。

返回到 runtime 之前不能调用调度器、syscall handler 或回收用户内存。底层记录不从用户寄存器取得可信指针，不允许 EL0 伪造内核 SP 或返回地址。

## Rust 执行循环

```rust
// 结构示意，不是可直接编译的实现。
loop {
    let Some(mut active) = scheduler.take_next() else {
        irq::wait_and_service();
        scheduler.wake_expired();
        continue;
    };
    let event = active.run();
    // 已回到 EL1、IRQ masked、内核空 TTBR0。
    let event = service_arch_event(event); // IRQ: ack、清源、EOI
    scheduler.complete_run(active, event); // 消费 ActiveRun
}
```

每次循环都在前一次 `run()` 返回后重新进入，调用深度不随 trap 次数增加。初次进入循环时保留的引导栈帧也是固定数量；不允许用不断递归调用 runtime 的方式模拟循环。

为了让本轮只改变控制流，先保留当前成功 syscall 后轮转的行为，不同时改变公平性策略。以后可以明确引入 ContinueCurrent 和 Reschedule，但必须另外验证延迟和公平性。

## 系统调用、等待与退出

系统调用处理不能再切换 CPU 上下文或直接进入下一个用户任务。它返回明确结果，由调度器提交：

| 结果 | 提交动作 |
| --- | --- |
| 完成 | 写回 x0，必要时写 x1；根据调用结果设置 Ready/Suspended/Sleeping 等状态 |
| wait 尚未完成 | 保存等待目标，进入 Waiting；最终返回值由目标终止事件填写 |
| exit | 记录终止结果，释放用户空间，唤醒等待者，不再恢复此上下文 |
| 普通错误 | 写回错误码，保留其他用户寄存器，按原策略继续调度 |

sleep/suspend-self 可以在停车前预写其成功返回值；wait 必须区分“调用已完成”和“尚待结果”，避免给用户暴露伪造结果。任务暂停时保留原等待状态，恢复时继续原等待；终止事件发生在暂停期间，也必须保存最终结果而不错误入队。

故障、正常退出、外部 destroy 共用明确的终止提交逻辑。只有切回内核 TTBR0、停止使用该空间且不存在借用时，才允许释放帧和页表。释放地址空间与保留 exit/fault 结果分离，维持现有 wait 后 destroy 的接口。ActiveRun 归还或丢弃后不得再次访问已回收任务槽。

将来的阻塞 IPC 同样保存等待对象和完成所需的数据，不保留栈上用户指针借用或跨任务可变引用。若某类操作必须保存多步执行状态，使用显式 continuation 数据；不能悄悄在共享栈上切换任务。

## IRQ 与 idle

EL0 IRQ 只让 run 返回 Interrupt；runtime 在 IRQ 关闭状态调用 GIC 服务函数，完成 acknowledge、清除/重装来源和 EOI。该函数报告实际事件，不能把未知 IRQ 或 spurious IRQ 都当作时间片中断。

当前 QEMU AArch64 idle 路径保持 DAIF.I=1（IRQ 掩蔽），使用带适当屏障的 WFI 等待，再在 Rust 中检查并服务可交付的 pending IRQ。AArch64 WFI 可被 DAIF 掩蔽的 IRQ 唤醒；GIC 的优先级/组使能仍必须允许该中断送达 CPU。调度循环每次选择任务前检查到期等待者，无可运行任务才进入 WFI，返回后再次选择；伪唤醒则重试；不得依赖“开 IRQ → 普通 WFI”之间没有竞争窗口。

此方案使 idle 不需要重置栈或通过 EL1 IRQ 路径跳入任务。专项测试在实际 QEMU 配置上覆盖“中断已 pending 才执行 WFI”及“休眠后到期”两种情况。若平台要求另一种等待序列，应由 arch 层实现等价的无丢唤醒协议。

EL1 IRQ 向量仍需有明确行为：如未来开放内核 IRQ，只允许保存现场、服务并返回被中断的内核指令流，不能借用当前共享栈去运行另一个用户任务。当前不开放内核 IRQ；非预期进入属于应诊断的内核不变量破坏。EL1 故障继续无条件诊断并按现有策略关机。

## 必须保持的不变量

1. 同一 CPU 最多一个 ActiveRun；调度器不可同时持有其用户上下文。
2. IRQ masked 进入及返回 run；只有恢复用户 PSTATE 时开放用户 IRQ。
3. 不持有 SingleCore 借用跨越 run、idle 或任何可能开放 IRQ 的边界。
4. lower-EL trap 在返回前不调用调度器；EL1 异常不使用用户返回记录。
5. 每次 trap 消耗的栈空间在返回时完全回收；内核栈不能指向用户地址。
6. 地址空间先失活，之后才可以修改/释放其生命周期资源。
7. 用户未修改的 GPR、SP、PC 和 PSTATE 在适用的恢复路径中保持语义一致。
8. LOG=off 不改变执行行为；内核 panic/致命异常继续无条件打印。

## 验证

`make check` 包含完整 debug/release × LOG=off/info 回归，以及以下专项验证：

| 检查 | 覆盖 |
| --- | --- |
| `check_user_context.py` | 每种配置连续 2048 次返回，内核 x19..x29 哨兵、x30 返回地址、SP 与 IRQ 状态；纯 EL0 死循环的定时器返回；在 masked WFI 指令处设置 pending timer 的唤醒 |
| `check_tasks.py` | 内存事务和失败回滚、任务授权、暂停恢复、等待、退出、故障回收、timer-only 双任务抢占、sleep 唤醒 idle |
| `check_fatboot.py` | root 栈保护初始化、hello、SVC 用户寄存器保存、读写执行权限、用户故障隔离 |
| `check_kernel.py` | 内核启动、页表权限、分配器、LOG 过滤、EL1 故障/panic 无条件诊断与 PSCI 关机 |
| `check_relocation.py` | 内核物理位置和 root 虚拟布局变化后上述任务执行仍然有效 |

`root_idle(root_state)` 是可返回的调试边界，在 x0 发布 root 状态；测试不再读取 Scheduler 对象布局。`run_user` 是实际架构调用入口的符号，用于直接检查调用者现场。没有为测试保留旧执行路径。

用户 ABI 不变；能力系统、IPC、TLS 和 FP/SIMD 支持不属于本轮控制流重构。

## 后续扩展边界

当需要独立内核线程或同步阻塞内核调用时，再增加 KernelContext、每任务 KernelStack 和内核任务调度器；届时每个用户线程可以拥有自己的 run_user_thread_loop，接近 x-kernel 的完整模式。必须额外设计栈映射、不可移动的上下文存储、挂起期间借用约束，以及切离退出任务栈后再释放栈的回收协议。

这些并不是现在让 run 可返回的前置条件。现阶段的完成标准是架构层交还事件、Rust 循环拥有调度控制流，且现有微内核行为保持完整。
