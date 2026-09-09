# 单核用户内存与任务调度

本实现参考 `/root/codes/x-kernel/mm/memspace` 的地址空间所有权，以及 `task/ktask` 的任务状态、运行队列和生命周期分离方式。没有直接移植其 Linux VMA、文件映射、进程或 SMP 子系统。当前目标是固定 QEMU virt 平台上的用户页管理与单核任务执行闭环。

## 模块边界

| 模块 | 负责内容 |
| --- | --- |
| `kernel/src/memory/frame.rs` | 物理帧唯一所有权、分配清零、释放和余量统计 |
| `kernel/src/memory/space.rs` | 地址空间、页元数据、私有页表、map/unmap/protect、受控跨页复制 |
| `kernel/src/boot.rs` | 接管 loader 已装载页、初始 BootInfo/IPC buffer |
| `kernel/src/task/scheduler.rs` | 任务上下文所有权、状态转换、等待关系、选择与回收 |
| `kernel/src/task/runtime.rs` | 每任务用户执行循环及创建时注入的 syscall 回调 |
| `kernel/src/task/queue.rs` | 有界 FIFO 就绪队列，不在调度路径分配内存 |
| `kernel/src/task/api.rs` | 授权的任务操作、地址空间访问和生命周期检查；任务表字段保持私有 |
| `kernel/src/syscall/dispatch.rs` | ABI 解码、handler 路由与即时/延迟结果编码 |
| `kernel/src/syscall/task.rs`、`memory.rs` | 任务与内存调用处理，返回显式调度决定 |
| `kernel/src/arch/irq.rs` | 通过 arm-gic-driver 配置 GICv3、PPI 30、物理定时器重装与中断完成 |
| `projects/libs/user` | `Task`、`TaskState`、`Permissions`、错误类型及系统调用封装 |

一个已启动任务包含一个用户地址空间、用户上下文、独立内核栈及可恢复的内核上下文。没有共享地址空间的多线程，也没有通用内核线程。这里的任务句柄是受父子所有权约束的内核对象引用，尚不是完整的 seL4 CSpace/capability 派生和撤销系统。

## 用户内存

内核通过 TTBR1 保留高地址 supervisor-only 映射：内核镜像位于 `0xffff800000000000` 窗口，以启动时发现的 PA 建立映射；物理直接映射仍使用固定偏移 `0xffff000000000000`。帧分配器统一使用直接映射地址，避免将镜像 VA 当成固定偏移别名。每个用户地址空间使用独立 TTBR0，分配独立 L0/L1/L2，按需分配用户 L3。用户地址窗口为 `0x1000..0x08000000`，因此不能映射内核 RAM、GIC 或 UART。

映射粒度为 4 KiB，只接受 R/NX、RW/NX、RX，拒绝 RWX、写而不可读、未知权限位、非对齐范围、溢出和重叠。用户页具有 EL0 AP 和 PXN；EL0 无法访问用于初始化的内核物理页别名。

资源由链接脚本显式预留：

- 内核元数据堆 16 MiB，与用户页分配池分离。
- 主用户帧池 8 MiB，共 2048 帧；另接管 loader 装载的实际 root image 区间，跨度最多 1024 帧。已映射页由 root 持有，空洞在接管完成后可分配；释放后两池均可复用。用户数据和私有页表都计入池配额。
- 每个地址空间最多 1024 个用户映射页；页表帧另行计费。
- 最多 32 个任务，包括 fatboot。耗尽时返回 NoMemory，不以用户输入触发内核 panic。
- 引导/调度器栈为 64 KiB；每个已启动任务另从内核堆分配 64 KiB 栈，均带下方 4 KiB 保护页。任务栈保护页的镜像和物理别名均取消映射，销毁后恢复并释放。堆、帧池和引导栈均为 NOLOAD，不增大磁盘启动镜像。

这些是当前固定平台的明确配额，不表示已发现或分配 QEMU 的全部 128 MiB RAM。内核不在运行时解析 DTB；PSCI、MMIO 和受支持 RAM 参数在构建时生成。运行期内存发现、完整保留区处理和释放剩余 RAM 尚未实现。loader 的 DTB 和保留程序头页不加入帧池；DTB 另复制到用户只读的扩展 BootInfo。

`map` 先检查整个范围，预留元数据容量并分配所有帧/页表，成功后才发布描述符。任何中途失败都会通过所有权析构归还暂存帧。`unmap` 先完整检查范围和固定页，再清除描述符、执行屏障和 TLB 失效，随后释放页及空 L3 表。`protect` 执行 break-before-make；授予执行权时维护 D-cache/I-cache 一致性。重新分配的帧先清零，避免跨任务遗留数据。

每次进入内核调度路径先切回内核页表；释放任务地址空间前确保其不再活跃。单核版本在切换时使用完整 TLB 失效，暂不分配 ASID，也不需要跨 CPU shootdown。

用户传来的地址不会直接构造成内核 Rust 引用。复制操作先验证完整范围和权限，再通过所属物理帧的内核别名复制；跨页失败不会留下部分写入。当前每次复制最多 4096 字节。BootInfo、包含 DTB 的扩展 BootInfo 和初始 IPC buffer 被固定，用户不能 unmap/protect，以维持运行库已建立的借用和启动约定。ELF 栈属于用户运行库，和其他自有映射一样可以按规则修改；调用者必须保证当前执行栈有效。

## 调度与异常

就绪任务进入 FIFO 轮转队列。当前每次用户系统调用返回都是调度点；主动 Yield、阻塞操作和 10 ms 周期定时器均可切换任务。两个完全不调用系统调用的 EL0 任务也会被定时器抢占。

任务状态：

| 状态 | 编码 | 含义 |
| --- | --- | --- |
| Created | 0 | 新建、尚未启动；允许装载内存 |
| Running | 1 | 当前执行者，不在就绪队列 |
| Suspended | 2 | 显式暂停，保留恢复所需上下文和阻塞原因 |
| Faulted | 3 | 用户不可恢复异常，空间已释放，保留结果等待回收 |
| Ready | 4 | 在就绪队列中且只出现一次 |
| Sleeping | 5 | 等待绝对计数器期限 |
| Exited | 6 | 已退出，空间已释放，保留退出码等待回收 |
| Waiting | 7 | 等待直接子任务退出或故障 |

创建任务只生成空地址空间。调用者先映射内存、写入代码/数据、设定 RX 权限和栈，再 Start。Start 校验可执行入口、4 字节 PC 对齐、16 字节栈对齐以及栈顶下的可写区域；x0 为启动参数，其余 GPR 清零，初始 PSTATE 为 EL0t，IRQ 开启。任意代码入口必须遵守装载程序自己的运行时约定，不能把未初始化的普通 Rust 函数当作完整应用。

暂停会移除就绪队列成员。暂停 Sleeping/Waiting 任务时保留原有期限或等待关系；恢复后继续阻塞，已经满足条件时才进入 Ready。等待对象在等待者暂停期间退出，也会保存正确结果供后续恢复。

Wait 在目标未终止时阻塞，结束后返回退出码或 ESR；通过 Status 区分 Exited 与 Faulted。Wait 不自动销毁句柄，Destroy 负责回收任务槽。Destroy 也可强制终止子任务，会先移出队列、释放地址空间并唤醒等待者。任务 ID 包含递增代数，槽复用后旧句柄失效。

父任务退出后，子任务转交仍存活的 root task。root 已终止时子任务成为无父任务，终止后自动回收非 root 槽；仍运行或显式暂停的孤儿保有自己的资源。root 本身的终止记录保留供诊断。

所有任务都只能操作自身或直接子任务。不能用猜出的 ID 操作父任务、兄弟任务或尚未转交的孙任务；根任务也没有绕过该规则的任意目标访问。修改另一个任务的空间要求目标为 Created/Suspended。IPC、句柄转移和能力授权以后另行设计。

没有 Ready 任务时内核在现有调用栈上、IRQ 掩蔽状态执行 WFI；pending timer 唤醒后由 Rust 处理 IRQ，再检查到期任务。用户异常保存到 `LAST_FAULT`，终止该任务并调度其他任务。内核自身异常或 panic 无条件诊断并 PSCI 关机。CNTKCTL_EL1 禁止 EL0 修改定时器。FP/SIMD 通过 CPACR_EL1 显式禁止，相关用户指令产生故障；尚无 FP/SIMD 上下文保存。

EL1 内核执行期间 IRQ 屏蔽、不可抢占，任务通过显式 park 切回调度器。独立内核栈保留阻塞的 Rust 调用链，wait 在唤醒后从原 handler 调用点继续。任何共享状态借用都必须在切换前结束；强制销毁不会展开挂起栈，所以持久资源必须由任务对象或入口捕获持有。它不是硬实时实现：映射清零、复制和元数据操作会增加中断响应延迟。

调度器、帧池、堆和 GIC 状态通过内核内部的 `SingleCore` 封装访问。访问时检查 IRQ 已屏蔽，并使用非原子的借用标记防止可变引用重叠；不等待，不切换 IRQ 状态，不使用自旋锁。借用必须在 eret、内核上下文切换或 idle 前结束，持有借用期间不得打开 IRQ。日志使用可失败的借用检查防止重入，panic 绕过它独立输出。此机制依赖单核执行约定，不支持 SMP。

## 用户接口与 ABI

应用使用 `rstiny::Task`；入口仍是 `#[entry]`。例如创建和回收尚未启动的子任务：

```rust
let child = rstiny::Task::create()?;
assert_eq!(child.status()?, rstiny::TaskState::Created);
child.destroy()?;
rstiny::sleep(20)?;
```

map/unmap/protect、写入任意目标地址及 Start 为 unsafe API，因为内核页权限不能代替调用者的 Rust 指针、引用和执行栈安全义务。`read_memory` 使用受控可变切片；句柄的权限仍由内核逐次检查。

`svc #0`：x8 调用号，x0..x4 参数，x0 返回状态；有返回值的操作用 x1 返回值，其余操作保留 x1。x2..x30 和用户 SP 保留。原有 0..2 调用号不变，BootInfo 二进制布局不变。

| 号 | 操作 | 参数 | x1 返回值 |
| --- | --- | --- | --- |
| 0 | Yield | 无 | — |
| 1 | DebugPutChar | 字节 | — |
| 2 | SuspendSelf | 无 | — |
| 3 | TaskId | 无 | 当前句柄 |
| 4 | TaskCreate | 无 | 子句柄 |
| 5 | TaskStart | 句柄、PC、SP、x0 参数 | — |
| 6 / 7 / 8 | Suspend / Resume / Destroy | 句柄 | — |
| 9 | TaskStatus | 句柄 | 状态 |
| 10 | Exit | 退出码 | 不返回 |
| 11 | Sleep | 毫秒 | — |
| 12 | Map | 句柄、地址、字节数、权限 | — |
| 13 | Unmap | 句柄、地址、字节数 | — |
| 14 | Protect | 句柄、地址、字节数、权限 | — |
| 15 / 16 | WriteMemory / ReadMemory | 句柄、目标空间地址、调用者缓冲区、字节数 | — |
| 17 | MemoryAvailable | 无 | 全局空闲帧数 |
| 18 | Clock | 无 | 单调毫秒计数 |
| 19 | Wait | 子句柄 | 退出码或 ESR |

错误状态：0=OK、1=Unsupported、2=InvalidArgument、3=NoMemory、4=NotMapped、5=AlreadyMapped、6=PermissionDenied、7=NotFound、8=InvalidState、9=Busy。内存权限位是 R=1、W=2、X=4。

## 验证与尚未实现的能力

`make check` 包含宏测试、ELF 输入校验器、原有内核回归、fatboot 回归，以及 `tools/check_tasks.py`。后者在 debug/release × LOG=off/info 配置下以固定 EL1 入口运行：

- 真实 EL0 系统调用、非法参数/地址/权限、BootInfo 固定页、过期和越权句柄。
- 跨页读写、失败无部分写入、unmap/remap 清零、任务销毁后帧与页表回收。
- 用户配额、全局内存池和任务槽耗尽，验证失败回滚和后续恢复。
- 两个无 Yield 循环的定时器抢占、相同 VA 的不同物理内容、暂停/恢复、idle 睡眠唤醒。
- 阻塞等待、暂停中的睡眠/等待、子任务转交、退出/故障后空间释放和句柄复用。
- 实际执行 EL0 定时器控制写入和 FP/SIMD 指令，验证权限陷入与故障隔离。

fatboot 也通过实际 Rust API 检查任务创建/销毁计费和定时器睡眠，保留静默启动结果验证。

当前已实现上述单核内存与任务接口的完整生命周期。未实现 SMP、共享地址空间线程、优先级/实时调度、需求分页、COW/fork、文件映射、swap、POSIX、通用 ELF exec、IPC 和完整 capability 系统；这些需要各自的用户接口及资源语义，不能从本次实现推断已具备。

执行模型见 [用户态执行控制权反转设计](user-execution.md)。当前实现采用可返回的用户执行边界、每任务独立内核栈和内核 continuation 调度。

Syscall 回调在创建任务入口时注入，由每任务 runtime 调用；任务模块不引用具体 syscall 实现。调度器仅接收 Resume/Suspend/Sleep/Wait/Exit/Fault 决定，不解释调用号或写 x0/x1。wait 完成值保存在任务中，恢复时由 syscall 层编码；暂停期间发生完成也不会自动恢复任务。

当前调用者由内核当前任务设施确定，syscall 与任务操作 API 不接受 caller 参数。`task::current_id()` 只返回 ID 副本，boot/idle 返回 None；所有目标句柄仍执行 generation 和父子授权检查。
