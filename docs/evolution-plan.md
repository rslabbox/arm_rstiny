# ARM RSTiny 能力系统与 IPC 演进规划

日期：2026-09-09。状态：设计提案，尚未实施。

本文是对当前单核进程内核的差距分析与修复规划。它不替代 [完整微内核设计与分阶段路线](microkernel-design.md)，而是把那条路线落成可执行的设计：给出对象模型、capability/Untyped/IPC/fault/IRQ 的具体语义、ABI 变更、模块改动、每阶段验收标准和风险。现状事实以 [内核实现与验证记录](kernel-implementation.md)、[用户内存与单核任务调度](memory-task.md)、[fatboot 启动与用户态边界](fatboot.md) 和源码为准。

当前项目已经具备：独立 TTBR0 用户地址空间、W^X 页权限、可回收帧、32 个任务槽、10 ms 定时器抢占、跨空间受控复制、用户故障隔离。它仍不是 seL4 语义的微内核，核心缺口是 **capability/CSpace、Untyped、IPC、fault endpoint、用户 IRQ**，以及支撑这些机制的 **每线程内核栈、优先级调度和资源计费**。本文按"先补对象与能力，再做 IPC 与故障，最后做驱动与优化"的顺序安排。

## 1. 结论与目标

### 1.1 差距分级

| 级别 | 差距 | 证据 | 影响 |
| --- | --- | --- | --- |
| P0 | 没有 capability/CSpace，只有父子关系授权 | `kernel/src/task/mod.rs` 的 `lookup()` 仅比较 `parent` | 无权限位、无派生/撤销、ambient authority |
| P0 | 没有 IPC（Endpoint/Notification） | 全仓库无 endpoint 对象；IPC buffer 未使用 | 无法构建用户态服务，微内核不成立 |
| P0 | 没有 Untyped/Retype，内核直接发页 | `kernel/src/memory/frame.rs`、`SYS_MAP` | 用户无法管理内存，无设备内存授权，无计费 |
| P0 | 内核栈全局唯一，syscall 不可阻塞 | `boot_stack_top`、`enter()`/`root_idle()` | 阻塞式 IPC 的前置重构 |
| P1 | 没有 fault endpoint，用户故障直接杀任务 | `kernel/src/arch/trap.rs` 的 `Event::Fault` | 无法容错/恢复，无 FAR/PC 交付 |
| P1 | 没有用户 IRQ 授权 | `kernel/src/arch/irq.rs` 屏蔽未知中断 | 用户态驱动不可能 |
| P1 | 没有优先级，纯 FIFO | `Task` 无 priority 字段 | 与设计文档的固定优先级描述不符 |
| P1 | 没有资源配额，子任务可耗尽全局池 | `SYS_TASK_CREATE`/`SYS_MAP` 无预算 | 单任务可饿死系统 |
| P2 | 无 ASID，每次切换全量 TLB 失效 | `memory::sync_translations` | 性能，IPC 延迟指标不可用 |
| P2 | 跨空间拷贝逐字节 + 二分 | `memory/space.rs` 的 `read/write` | 性能 |
| P2 | 启动依赖 loader 的 MAIR/TCR | `arch/boot.rs` 的 `enable_mmu` | 平台/loader 耦合脆弱 |
| P2 | ABI 无保留字段、无兼容读取 | `projects/libs/abi` | 前向兼容差 |
| P2 | `irq::handle()` 返回值被忽略 | `arch/trap.rs` | 非 timer 中断被当作调度点 |

### 1.2 第一版目标（本文的验收边界）

第一版目标是"最小可用 seL4 子集"：

```text
root task 持有初始 CNode 和全部 Untyped
  → Retype 出 Frame / PageTable / TCB / CNode / Endpoint / Notification
  → Frame_Map 建立地址空间，TCB_Configure/SetSpace/SetIPCBuffer
  → TCB_Resume 启动子任务
  → 两个任务通过 Endpoint 完成 Call/Reply 与 Notification
  → 子任务缺页/非法指令投递到 fault endpoint，监管者可修复或终止
  → 根任务从 IRQControl 派生 IRQHandler，绑定 Notification，用户态串口服务轮询 TX
```

不追求：SMP、MCS、虚拟化、动态链接、POSIX、形式化验证、seL4 二进制兼容。这些仍在 [完整微内核设计与分阶段路线](microkernel-design.md) 的后续里程碑中。

## 2. 总体设计原则

1. **能力即权限**：任何对象操作都必须携带一个指向该对象的 capability，内核在调用者 CSpace 中解析并检查类型与权限位。数字句柄不再是对象引用，猜 ID 不能获得权限。
2. **用户内存由 Untyped 提供**：内核不再为普通用户对象从全局堆/帧池直接发内存；内核只验证 Retype 并从 Untyped 切分。内核元数据使用有界预留区，不随用户请求无限增长。
3. **先校验后提交**：所有多步对象操作（Retype、Map、CapTransfer）必须能回滚，失败不留下半初始化对象或半次通信。
4. **阻塞必须有每线程栈**：一旦引入 Recv/Fault 阻塞，内核必须为每个 TCB 保存独立内核栈和挂起状态，禁止继续共用 `boot_stack`。
5. **未实现的操作显式失败**：所有 seL4 语义但本项目暂不支持的操作返回 `Unsupported` 或 `NotImplemented`，不得静默降级为"绕过权限"。
6. **ABI 版本化**：BootInfo 与 syscall 一起升到 v4；旧接口在迁移期保留为兼容 shim，迁移完成后删除。
7. **渐进可回归**：每个阶段结束时 `make check` 全绿，现有 EL0/内存/调度回归不得退化。

## 3. 对象模型与 capability

### 3.1 对象种类

| 对象 | 作用 | 第一版操作 |
| --- | --- | --- |
| Untyped | 一段物理内存的创建权 | Retype、Revoke |
| CNode | capability 槽表 | Copy、Mint、Move、Delete、Revoke |
| Frame | 4 KiB 物理页（普通或设备） | Map、Unmap、GetInfo |
| VSpace/PageTable | 用户地址空间根及页表 | Map、Unmap |
| TCB | 线程上下文与调度状态 | Configure、SetSpace、SetIPCBuffer、Read/WriteRegisters、SetFaultEndpoint、SetPriority、Resume、Suspend |
| Endpoint | 同步 IPC 队列 | Send、Recv、Call、Reply |
| Notification | 异步位通知 | Signal、Wait、Poll |
| IRQHandler | 中断线路授权 | SetNotification、Ack |

### 3.2 对象引用与生命周期

```rust
// kernel/src/object/ref.rs（建议）
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ObjRef {
    kind: u8,
    index: u16,   // 对象表中的槽
    generation: u32,
}
```

- 对象表是**有界**的固定数组（建议 4096 项，从内核预留元数据区分配，不来自全局堆）。每项是 `Option<Object>` 加 generation。
- 对象释放时 generation 递增；旧 `ObjRef` 因 generation 不匹配而被拒绝，杜绝槽复用后的 ABA。
- 对象负载（页表页、CNode 槽数组、TCB 栈）从 Untyped 切分；对象表本身只存索引与元数据。
- 对象表满时 `Retype` 返回 `NoMemory`，不允许触发内核 panic。

### 3.3 Capability 与 CNode

```rust
// kernel/src/object/cap.rs（建议）
bitflags::bitflags! {
    pub struct Rights: u64 {
        const READ       = 1 << 0;  // Frame 读 / CNode 读取槽
        const WRITE      = 1 << 1;  // Frame 写
        const EXECUTE    = 1 << 2;  // Frame 执行
        const GRANT      = 1 << 3;  // IPC 中转移该 cap
        const SEND       = 1 << 4;  // Endpoint 发送
        const RECV       = 1 << 5;  // Endpoint 接收
        const CALL       = 1 << 6;  // Endpoint Call
        const MANAGE     = 1 << 7;  // Retype/配置子对象
    }
}

#[derive(Clone, Copy)]
pub struct Cap {
    object: ObjRef,
    rights: Rights,
    badge: u64,        // Endpoint 投递时携带；非 Endpoint 为 0
    parent: u32,       // 派生来源槽号或 0xffff_ffff 表示直接持有
}
```

- **单层 CNode**：`CapPtr` 为 `u64`，低 16 位是槽号，高位保留（第一版必须为 0）。一个 CNode 由一页 Frame 承载，按 32 字节/cap 计算为 128 槽。多级 guard/radix 留待后续，文档必须标注与 seL4 的差异。
- **句柄不是地址**：用户传入槽号，内核在调用者 CNode 中查表。修改整数不会创建权限。
- **派生与撤销**：`CNode_Copy/Mint` 只能在权限子集内派生；`badge` 只能由 Endpoint 派生时设置。`CNode_Revoke` 删除该槽派生出的所有 capability。第一版用有界对象表扫描实现：`Revoke(Untyped)` 删除该 Untyped 派生的全部对象，`Revoke(CNode)` 清空该 CNode 及其后代槽。扫描 O(对象表) 是可接受的，因为表有界；后续再引入 seL4 风格的 derivation tree。
- **权限检查**：每个 syscall 先解析 cap，再检查 `kind` 与 `rights`。错误区分 `InvalidCapability`、`PermissionDenied`、`WrongType`。

### 3.4 Untyped 与对象存储

```rust
// kernel/src/object/untyped.rs（建议）
pub struct Untyped {
    phys: usize,      // 物理起点，size_bits 对齐
    size_bits: u8,    // 4..=30
    is_device: bool,  // 设备内存只能 retype 成 Frame
    free: usize,      // 当前未切分字节
    parent: Option<ObjRef>, // 指向父 Untyped，用于 Revoke
}
```

Retype 规则（逐条校验，失败前不产生半初始化对象）：

1. cap 类型为 Untyped 且持有 `MANAGE`；设备 Untyped 只能生成设备 Frame。
2. 目标类型的大小、对齐、`size_bits` 合法；数量 `count > 0` 且不溢出。
3. `count * object_size <= free`，且切分后剩余仍是合法对齐的 Untyped（或允许浪费并记录）。
4. 目标 CNode 的 `[dest_index, dest_index+count)` 全部为空槽。
5. 目标槽不跨越 CNode 边界。
6. 普通对象内存**清零**；设备 Frame 不清零且标记 `is_device`。
7. 全部校验通过后一次性提交：写对象表、切分 Untyped、安装 cap。任一步失败则回滚。

启动时把 RAM 按保留区（内核镜像、loader、DTB、内核元数据、初始对象）切分，剩余部分生成 Untyped cap 交给 root task；GIC、内核定时器不进设备 Untyped；UART/VirtIO 作为设备 Untyped 由 root 决定分给哪个驱动。

### 3.5 初始 capability 布局

BootInfo v4 需要发布：

- root task 的初始 CNode cap 槽号与空槽区间；
- 全部 Untyped 描述（物理地址、size_bits、is_device）；
- 初始 IPC buffer 地址；
- 可选：初始 TCB/VSpace 的槽位。

root task 启动时至少持有：自身 TCB、自身 VSpace、初始 CNode、全部可用 Untyped。它用这些能力构造子任务，而不是让内核直接替它 `SYS_TASK_CREATE`。

## 4. 地址空间与线程对象

### 4.1 VSpace / PageTable / Frame

- `VSpace` 是地址空间根对象，持有根页表 Frame；`PageTable` 中间级可选。
- `Frame_Map(frame_cap, vspace_cap, va, rights)` 校验 Frame 的 `READ/WRITE/EXECUTE` 与目标 VSpace 的 `MANAGE`，执行 break-before-make，并维护反向映射（每 Frame 记录已映射位置，第一版允许每 Frame 至多一处映射，简化 Unmap/Revoke）。
- `Frame_Unmap(frame_cap, vspace_cap, va)` 完成 TLB 失效后才归还 Frame 所有权。
- 设备 Frame 只能映射为 Device 属性，且不允许 `EXECUTE`。
- 用户页权限仍限制为 R/NX、RW/NX、RX；禁止 RWX。

### 4.2 TCB 与每线程内核栈

```rust
pub struct Tcb {
    state: ThreadState,           // Inactive/Running/Restart/Blocked*/Ready
    context: TrapFrame,
    cspace: ObjRef,
    vspace: ObjRef,
    ipc_buffer: (u64, Option<ObjRef>),
    fault_ep: Option<Cap>,
    reply_to: Option<ObjRef>,     // 一次性的 Call 回复对象（第一版）
    priority: u8,
    timeslice: u8,
    kernel_stack: ObjRef,         // 独立内核栈 Frame
    queue: QueueLink,
}
```

- **每 TCB 一条内核栈**（建议 8 KiB，从 Untyped 或内核元数据预留区分配）。异常入口先用一个极小的 per-CPU 引导栈切到当前 TCB 的栈，再保存 TrapFrame。
- 阻塞 syscall（Recv/Call/ReplyRecv/Fault）在保存上下文后不再返回用户态，而是调用调度器；被唤醒时从保存的 TrapFrame 恢复。因此内核调用栈不再跨阻塞存活。
- 这是 IPC 的硬前置条件。第 3 阶段的实现必须与内核栈重构一起提交。

### 4.3 调度器

- 每个优先级一条 FIFO 就绪队列；选择最高优先级，队内轮转；空则 idle。
- 第一版不做优先级继承/时间预算，必须在文档中明确"存在优先级反转与饥饿"。
- 普通线程只能在自己的授权优先级范围内设置子线程；不能提升到系统服务之上。
- 时间片按 TCB 配置，定时器中断递减；耗尽后移到同优先级队尾。

## 5. IPC

### 5.1 Endpoint 状态机

```rust
enum EndpointState {
    Idle,
    Send(Queue<TcbRef>),
    Recv(Queue<TcbRef>),
}
```

- `Send(ep)`：无接收者则阻塞入队；有接收者则直接交付并唤醒。
- `Recv(ep)`：无发送者则阻塞入队。
- `Call(ep, msg)`：等价 Send + 等待回复；调用者阻塞状态为 `BlockedReply`。
- `Reply(msg)`：消费一次性的回复关系，唤醒原调用者。
- 队列节点由 TCB 内嵌，不在 IPC 路径分配内存。

### 5.2 消息格式

- **短消息（fastpath）**：最多 4 个机器字走寄存器（label、length、mr0、mr1），长度上限写入 `info` 字段。
- **长消息**：通过调用者 IPC buffer 传递，长度上限建议 120 字节以内起步；cap 转移最多 4 个，必须来自 IPC buffer 且有 `GRANT`。
- 内核必须校验：长度上限、IPC buffer 已配置且映射、cap 数量上限、cap 类型/权限、目标槽为空。错误时不留下半次通信。
- 消息里自报的身份不可信；接收方用 Endpoint badge 识别调用者。

### 5.3 Reply 与防重放

- 第一版：接收者 TCB 内保存一个一次性 `reply_to` 槽，`Reply` 消费后清空，未回复前不可被覆盖；这比完整 reply capability 弱，文档必须标注。
- 后续升级为显式 reply capability：`Call` 授予接收者一个只能消费一次的 `Reply` cap；没有该 cap 无法唤醒调用者。

### 5.4 Notification

- `Notification { bits: u64, waiting: Queue<TcbRef> }`。
- `Signal(ntfn_cap, badge)`：`bits |= badge`，唤醒一个等待者；多次 Signal 合并。
- `Wait(ntfn)`：`bits != 0` 时原子取走并返回，否则阻塞；`Poll` 为非阻塞版本。
- 设备中断通过 Notification 投递，驱动必须读取设备直到清空源，不能假设一次 Signal 对应一次事件。

### 5.5 阻塞与唤醒的一致性

- 线程至多出现在一条等待队列中；状态与队列必须匹配。
- 对象删除/Revoke 时必须清理所有等待者并返回错误，不能永久悬挂。
- 暂停一个阻塞线程要保留阻塞原因；恢复后继续等待或立即完成，语义与现有 `suspended_from` 一致。

## 6. Fault endpoint

- TCB 配置 `fault_ep`。用户异常发生时，内核构造 fault 消息：类型（页故障/非法指令/未知 syscall/FP 陷入）、FAR、ELR、SPSR、通用寄存器、访问类型；把故障线程置为 `BlockedFault`，通过 fault endpoint 投递。
- 未配置 `fault_ep` 时终止线程（保留现有行为）。
- 监管者可用 `TCB_ReadRegisters/WriteRegisters`、`Frame_Map` 修复缺页、`TCB_Resume` 恢复，或 `TCB_Suspend` 终止。
- 恢复时必须校验 PC/SP/PSTATE，防止把线程恢复到内核态。
- 内核自身异常仍走独立 panic 路径，不投递给用户。
- `LAST_FAULT` 保留为调试用的最近一次故障记录，但不再是唯一诊断来源。

## 7. 用户 IRQ 与用户态驱动

- 根任务持有 `IRQControl` cap，可 `IRQControl_GetIRQHandler(irq_num)` 生成 `IRQHandler`；普通任务不能。
- `IRQHandler_SetNotification(handler, ntfn)` 绑定；内核在中断到来时 ack GIC、屏蔽线路、`Signal(ntfn, irq_num)`。
- 驱动处理完设备后 `IRQHandler_Ack(handler)` 才重新开放线路，防止电平中断风暴。
- 内核保留定时器与自身专用中断，拒绝重复或无权绑定。
- 第一个驱动是用户态 PL011 串口服务（轮询 TX，先不依赖 RX 中断）。`DebugPutChar` 在串口服务可用后降级为 `kernel-test`/调试专用，正式构建可由 root 决定是否发布。
- 明确边界：无 IOMMU/SMMU 时，持有总线主控设备的驱动属于受信任组件，不能宣称其崩溃一定被隔离。

## 8. ABI 与 BootInfo 演进

### 8.1 BootInfo v4

```rust
#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub version: u64,        // 4
    pub size: u64,
    pub page_size: u64,
    pub flags: u64,
    pub ipc_buffer: u64,
    pub extra: u64,
    pub extra_size: u64,
    pub init_cnode_slot: u64,
    pub untyped_count: u64,
    pub reserved: [u64; 6],
}
```

扩展记录（id/len 头沿用现有约定）：

| id | 记录 |
| --- | --- |
| 6 | FDT |
| 7 | Untyped 列表：`count` + 每项 `{ phys, size_bits, is_device }` |
| 8 | 初始 CNode/槽信息 |

读取规则：`size >= 已知字段` 即可读取已知字段，未知记录跳过；不得因为多出未知字段而拒绝启动。这是当前版本缺少的前向兼容能力。

### 8.2 syscall 约定

- 保持 `x8` 调用号、`x0..x4` 参数、`x0` 状态、有返回值用 `x1`；新增能力参数一律是槽号，不是对象地址。
- 建议按对象分组编号（示意，最终以 `projects/libs/abi` 为准）：

| 组 | 调用 |
| --- | --- |
| 0x10 | UntypedRetype |
| 0x20 | CNodeCopy / CNodeMint / CNodeMove / CNodeDelete / CNodeRevoke |
| 0x30 | FrameMap / FrameUnmap / FrameGetInfo |
| 0x40 | TCBConfigure / TCBSetSpace / TCBSetIPCBuffer / TCBReadRegisters / TCBWriteRegisters / TCBSetFaultEndpoint / TCBSetPriority / TCBResume / TCBSuspend |
| 0x50 | Send / Recv / Call / ReplyRecv |
| 0x60 | Signal / Wait / Poll |
| 0x70 | IRQGetHandler / IRQSetNotification / IRQAck |
| 0x80 | Yield / DebugPutChar（兼容） |

- 错误码扩展：`InvalidCapability`、`WrongType`、`RangeError`、`TruncatedMessage`、`FailedLookup`、`RevokeFailed`、`DeleteFirst`。现有 0..9 保持含义不变。

### 8.3 迁移策略

- 阶段 1 起新增能力接口，但**保留现有 task/memory syscall** 作为兼容 shim，避免一次性破坏 `tools/check_tasks.py` 和 fatboot。
- shim 内部改为通过 root 的初始能力实现，语义不弱化。
- 阶段 4（fault endpoint 完成）后，把 fatboot/hello 和测试迁移到能力接口，再删除 shim 与旧 BootInfo 字段。
- `ABI_VERSION` 一次性升到 4；旧版本用户程序不保证可运行。

## 9. 分阶段实施计划

### 阶段 0：工程修复（无 ABI 变化）

目标：消除 P2 中不涉及 ABI 的问题，为后续重构降低风险。

- `arch/trap.rs` 使用 `irq::handle()` 的返回值，只有 timer 才触发调度决策。
- 跨空间 `read/write` 改为按页 `copy_nonoverlapping`，`MAX_COPY` 提到一页并按需循环；保持"失败无部分写入"。
- 内核自己写 `MAIR_EL1`/`TCR_EL1`（或至少校验关键字段），不再隐式继承 loader 配置。
- `Task.state` 改为 enum，去掉 `Task.root` 冗余字段。
- `BootInfo` 加 `reserved`/`flags`，ABI 保持 v3 但预留字段。
- 明确"所有用户可触发的分配必须 fallible"，加测试或 lint 守住。

验收：现有 `make check` 全绿；新增非 timer 中断不触发调度的断言；页拷贝回归保持通过。

风险：低。可独立提交。

### 阶段 1：对象、CNode、Untyped、Frame

目标：把 `Task`/`AddressSpace`/`Frame` 改成可被 capability 引用的对象。

- 新增 `kernel/src/object/`：`ref.rs`、`cap.rs`、`cnode.rs`、`untyped.rs`、`frame.rs`、`table.rs`（有界对象表）。
- `Frame::allocate` 仅供内核元数据与启动期使用；用户 Frame 只能来自 `UntypedRetype`。
- 实现 `CNodeCopy/Mint/Move/Delete/Revoke` 与 `UntypedRetype`，全部先校验后提交。
- BootInfo v4 发布初始 CNode 与 Untyped 列表；root task 启动时获得这些 cap。
- 兼容 shim：旧 `SYS_MAP` 内部从 root 的 Untyped 切 Frame 再映射。
- 撤销：`Revoke(Untyped)` 回收其派生的对象；`Revoke(CNode)` 清空后代槽。

验收（宿主 + QEMU）：

- 伪造槽号、错误类型、只读权写映射、覆盖非空槽、数量溢出、目标跨界、设备 Untyped 生成普通对象全部失败。
- 合法 Retype/Map/Unmap/Revoke 成功；撤销后旧 cap 失效；对象表/Untyped 耗尽返回 `NoMemory` 而非 panic。
- 新页清零；回收后复用不泄漏旧数据。

风险：中。对象表与 `AddressSpace` 的所有权改造较大，需保持现有内存测试。

### 阶段 2：TCB/VSpace 分离、每线程内核栈、优先级调度

目标：把"任务"拆成 TCB 与 VSpace，并让内核可以阻塞。

- `Tcb` 持有 `cspace`/`vspace`/`ipc_buffer`/`fault_ep`/`priority`/独立内核栈。
- 异常入口切到当前 TCB 内核栈；阻塞 syscall 保存上下文后直接调度，不跨阻塞保留 Rust 栈。
- 每个 VSpace 可被多个 TCB 共享；同一 VSpace 下多线程共享地址空间。
- 调度器加优先级 + 队内轮转 + 可配置时间片。
- `SYS_TASK_*` shim 映射到 TCB/VSpace 操作。

验收：

- 两个 TCB 共享一个 VSpace，看到相同物理内容；不同 VSpace 同 VA 不别名。
- 忙循环低优先级线程不饿死高优先级；同优先级轮转。
- 内核栈互不覆盖；阻塞线程恢复后上下文正确。

风险：高。这是最容易引入调度/栈 bug 的阶段，必须新增 GDB 与 QEMU 并发测试。

### 阶段 3：Endpoint / Notification IPC

目标：实现同步与异步 IPC。

- Endpoint 状态机、Send/Recv/Call/ReplyRecv、Notification Signal/Wait/Poll。
- 短消息寄存器路径 + IPC buffer 长消息 + 受 `GRANT` 限制的 cap 转移。
- 一次性回复关系与防重放校验。
- 对象删除/Revoke 清理等待者。

验收：

- 双向 ping-pong、重复 Call/Reply、无权限 Endpoint、超长消息、非法 cap 转移、重复回复全部按预期成功或失败。
- 接收方退出/对象撤销时等待者被唤醒并收到错误，不悬挂。
- 用 IPC 实现一个最小 echo 服务，fatboot 通过它输出。

风险：高。需要与阶段 2 的内核栈一起设计；消息 ABI 一旦发布难以更改。

### 阶段 4：Fault endpoint 与监管者

目标：用户故障可投递、可恢复。

- 故障消息格式与 `BlockedFault` 状态。
- 监管者读取/写回寄存器、修复缺页、恢复或终止。
- fatboot 作为监管者处理 hello 的页故障并演示恢复一次。

验收：

- 缺页被监管者补映射后线程继续执行；无法修复时被终止且其他任务存活。
- 未配置 fault endpoint 时行为与现状一致。
- 恢复路径校验 PC/SP/PSTATE，不能恢复到内核态。

风险：中。

### 阶段 5：IRQ 授权与用户态串口

目标：用户态驱动闭环。

- `IRQControl`/`IRQHandler` 对象与 GIC ack/mask/notify/Ack 流程。
- 用户态 PL011 串口服务（轮询 TX）通过 Notification 或 Endpoint 提供 `ConsoleWrite`。
- `DebugPutChar` 降级为调试专用；正式构建可关闭。
- 设备 Untyped 只映射给对应驱动。

验收：

- `LOG=off` 下 root 与测试客户端仍能经用户服务输出。
- 普通客户端没有 UART 映射，直接访问会故障。
- 电平中断在源未清除时不风暴；Ack 后才能再次投递。

风险：中。

### 阶段 6：性能与后续

- ASID 分配与按 ASID 失效，替换全量 TLB 失效。
- IPC fastpath、批量日志、映射开销测量。
- 再评估 SMP、MCS、SMMU、FP/SIMD 上下文，各自独立里程碑。

验收：给出 IPC 延迟、上下文切换、映射与镜像尺寸基线；现有回归不退化。

风险：低到中。

## 10. 测试与验证计划

| 层 | 新增检查 |
| --- | --- |
| 宿主单元 | capability 派生/撤销状态机、Untyped 切分算术与溢出、CNode 边界、Endpoint 队列状态机、消息长度/cap 转移校验、fault 消息编码 |
| QEMU 集成 | Retype/Map/Resume、cap 伪造与权限、Revoke 后旧引用失效、IPC ping-pong、Notification 合并、fault 修复与终止、IRQ 投递与 Ack、资源耗尽返回错误 |
| 端到端 | 用户态串口服务、`LOG=off` 输出、替换磁盘/镜像程序、服务故障恢复 |
| 回归 | 保留现有 `tools/check_kernel.py`、`check_fatboot.py`、`check_tasks.py`、`check_bootloader.py`、`check_relocation.py` |

每个阶段必须同时提供正常用例和权限/失败用例，不能只观察一行输出。测试 harness 要区分"预期阻塞""panic 停机""死循环"。

## 11. 明确不做与延后

- 不做 seL4 二进制兼容；不继承其形式化证明。
- 不做多级 CNode guard/radix，第一版单层。
- 不做完整 derivation tree 撤销，第一版用有界对象表扫描。
- 不做 reply capability 的安全强化，第一版用一次性回复槽。
- 不做优先级继承、MCS、超时 IPC、SMP、SMMU、FP/SIMD 上下文。
- 不做通用 `open/read/write/fork/exec`，服务组合通过对象与 IPC 完成。

## 12. 里程碑与优先级总表

| 阶段 | 交付 | 依赖 | 退出条件 |
| --- | --- | --- | --- |
| 0 | 工程修复 | 现状 | `make check` 全绿，P2 项清零 |
| 1 | 对象 + CNode + Untyped + Frame | 0 | 能力伪造/撤销/耗尽测试通过 |
| 2 | TCB/VSpace + 内核栈 + 优先级 | 1 | 共享 VSpace、抢占、阻塞恢复通过 |
| 3 | Endpoint/Notification IPC | 2 | 双向 IPC + 错误路径通过 |
| 4 | Fault endpoint | 3 | 缺页修复与终止通过 |
| 5 | IRQ + 用户串口 | 4 | `LOG=off` 用户服务输出通过 |
| 6 | ASID/fastpath/性能 | 5 | 基线测量 + 回归 |

建议的提交顺序：阶段 0 单独提交；阶段 1 先做对象表与 Untyped 再做 CNode 操作；阶段 2 的内核栈重构与阶段 3 的 IPC 设计评审合并进行，避免两次返工。每阶段结束更新 [内核实现与验证记录](kernel-implementation.md) 和本文状态。
