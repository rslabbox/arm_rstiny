# 用户任务执行模型

状态：已实现，固定 AArch64 QEMU virt、单 CPU。

## 任务创建与控制反转

参考 x-kernel 的 `posix/process/src/runtime.rs::new_user_task`：创建方提供用户上下文和 syscall 回调，构造一个拥有独立内核栈的可调度任务。每个任务的入口运行自己的用户循环；调度器只保存、恢复内核 continuation，不解释用户陷入或 syscall 编号。

本项目的入口构造为：

```rust
fn new_user_task(
    mut uctx: UserContext,
    mut dispatch_syscall: impl FnMut(&mut UserContext) -> Disposition + Send + 'static,
) -> Result<Execution, Error> {
    Execution::new(move || {
        run_user_thread_loop(&mut uctx, &mut dispatch_syscall);
    })
}
```

root 由 `boot.rs` 注入 `syscall::dispatch`；子任务由 syscall 层在 TaskStart 时注入同一处理器。TaskCreate 先预留句柄和空地址空间，调用者完成装载后才启动，因此内核栈和入口在 TaskStart 时创建。校验或内存分配失败时任务保持 Created，局部分配由所有权回收，可以重试。

回调保存在任务拥有的稳定堆分配中，支持带状态的 `FnMut`。任务执行模块不引用具体 syscall 模块，也没有全局处理器注册表。Linux clone 的 set_child_tid、进程/线程拆分、信号及 POSIX ABI 不在本项目的接口中。

## 模块边界

| 模块 | 职责 |
| --- | --- |
| `boot.rs` | 接管 root 镜像、建立 BootInfo，组装首个用户任务 |
| `task/runtime.rs` | new_user_task 和每任务 run_user_thread_loop，调用注入的 syscall 策略 |
| `task/execution.rs` | 稳定入口闭包、内核上下文和内核栈的唯一所有权 |
| `task/stack.rs` | 可失败的内核栈分配、保护页及回收 |
| `task/scheduler.rs` | 内核 continuation 调度、park、状态转换、等待关系、idle 和回收 |
| `task/api.rs` | 当前调用者身份、目标授权与地址空间操作 |
| `arch/kernel_context.rs` | IRQ 屏蔽状态下切换内核 callee-saved 寄存器和 SP |
| `arch/user.rs`、`arch/trap.rs` | 进入 EL0、返回陷入事件及致命异常诊断 |
| `syscall/dispatch.rs` | 枚举分发、参数解码和 ABI 结果写回 |

## 每个任务的运行循环

循环在任务自己的内核栈上执行：

1. 在短期调度器借用内读取当前任务的页表根，结束借用。
2. 调用 `uctx.run(root)`，进入 EL0。
3. 陷入返回同一 Rust 调用点；此时 IRQ 屏蔽，TTBR0 已恢复为空内核根。
4. Syscall 交给注入的回调；IRQ 完成 ack/清源/EOI；用户故障记录后请求终止当前任务。
5. 调用 `park(action)` 切回调度器栈。再次被选中时从该调用后继续循环。

非定时器或伪中断直接继续当前任务。保留现有 FIFO 与正常 syscall 后轮转的策略；10 ms 定时器可抢占不主动 yield 的 EL0 程序。EL1 期间 IRQ 始终屏蔽，不支持内核抢占。

`wait` 可以在 syscall handler 内调用 `park(Wait(target))`。调度器记录等待关系，子任务终止时保存完成值并唤醒等待者。等待者恢复自己的内核调用栈，`park` 返回完成值，handler 返回，dispatch 写回 x0/x1。等待者暂停期间的完成只记录结果，不自动恢复它。查询完成状态与提交等待之间不会运行其他任务，避免丢失唤醒。

Sleep/Suspend 在用户循环提交对应动作；Exit/Fault 切走后永久不再返回该 continuation。调度器本身不编码 syscall 返回寄存器。

## 三种上下文与栈

### UserContext

保存 EL0 的 x0..x30、SP_EL0、ELR_EL1、SPSR_EL1，复用 272 字节 TrapFrame。它持久存在于任务入口闭包的捕获中，通过可变借用传给运行循环和 syscall handler。任务运行期间其他任务不能访问或替换它。

用户 ABI 使用 softfloat。FP/SIMD 通过 CPACR_EL1 禁止，TLS 尚未实现。

### KernelReturnFrame

一次 `UserContext::run` 在任务内核栈上创建的临时记录，保存内核 x19..x30 和可信的上下文/陷入缓冲区指针，共 112 字节。lower-EL 向量保存完整用户现场和 ESR/FAR，然后恢复该记录，通过 ret 返回 Rust。每次 run 返回时完全回收，调用深度不随陷入次数增长。

SVC #0 作为正常 Syscall 返回，未知调用号由 dispatch 拒绝。EL1 异常、FIQ、SError 走内核致命诊断；EL0 同步故障只终止对应任务。

### KernelContext 与 Execution

KernelContext 保存内核 x19..x30 和 SP，按 16 字节对齐，共 112 字节。首次恢复时，x19 指向可信入口捕获，x30 指向入口 trampoline，SP 指向独立栈顶。后续恢复返回先前的 park 调用。

Execution 拥有稳定堆分配的入口捕获和 64 KiB 内核栈。栈下方额外分配一个 4 KiB 保护页；该页在内核镜像映射和物理直接映射中都取消映射。释放前恢复两处映射，再交回堆分配器。栈来自现有 16 MiB 内核堆，不占用用户帧池。

原有带保护页的 64 KiB 引导栈承载调度器与 idle。调度器在一次执行期间将 Execution 从任务槽移出，任务槽标记 Running；切回后核对带 generation 的 ID，再归还或销毁。稳定入口捕获的地址不随 Execution 所有者移动而改变。

## 切换、销毁与借用约束

- 当前任务身份由 `task::current_id() -> Option<u64>` 提供。boot/idle 返回 None；syscall 与 task API 不接受可指定 caller。
- 一次只运行一个内核 continuation。切换前必须回到空 TTBR0，结束调度器、帧池、堆和 GIC 的所有共享状态借用。
- 调度器栈上的当前执行对象与调度器 KernelContext 在一次切换期间保持地址稳定。park 使用本轮调度器安装的切换链接；恢复后验证身份并取出等待完成值。
- 当前任务不能自行释放正在使用的栈。Exit/Fault 先切回调度器，调度器再销毁入口捕获、栈和地址空间。destroy 其他停止执行的任务可以立即回收。
- 强制销毁不展开被挂起的 Rust 栈。允许挂起的边界不能保留依赖 Drop 回收的栈局部资源或共享状态借用；持久资源由 Execution 的入口捕获及任务对象拥有。当前 wait 和用户循环满足这一约束。今后增加阻塞 handler 时也必须遵守。
- EL1 关闭 IRQ 只保证单核互斥，不保证硬实时延迟。SMP、内核抢占和任意位置取消需要新的同步及资源协议。

## 系统调用与用户指针

共享 ABI 库以 `#[repr(u64)] enum Syscall` 定义调用号，用户库只在 SVC 边界转换为整数。dispatch 通过 `TryFrom<u64>` 检查调用号，再穷尽匹配枚举，没有 route 层。

寄存器布局封装在 UserContext 的 `syscall_number()`、`arg0()..arg3()`、`set_syscall_result()` 中。指针参数通过 `uctx.argN().into()` 构造 `UserConstPtr<u8>` 或 `UserPtr<u8>`；转换不验证地址，也不创建 Rust 用户内存引用。task API 授权后在指定地址空间中验证完整范围、映射与权限，再通过内核持有的帧复制。源数据完整暂存后再写目标，支持重叠复制且失败不部分写入。

映射起点、任务入口与栈顶按地址处理，不作为普通数据指针解引用。

## 验证

`make check` 覆盖 debug/release × LOG=off/info：

- 2048 次 EL0 返回，验证内核 callee-saved 寄存器、SP、IRQ 状态和定时器独立返回。
- task → scheduler → task 的真实内核 continuation 切换与寄存器恢复，独立内核栈范围及双别名保护页。
- 31 个同时挂起的子任务及保护页回收，超过堆容量累计值的 260 次任务启动/退出/销毁。
- 当前任务身份、父子授权、跨页复制、内存耗尽回滚、定时器抢占、sleep、wait、暂停中的等待完成、销毁阻塞任务。
- masked WFI 前已有 pending timer 的唤醒、用户故障隔离、LOG=off、内核物理重定位和不同 root 虚拟地址。
