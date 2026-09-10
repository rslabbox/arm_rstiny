# 独立 fault-handler 线程与线程模型

日期：2026-09-12。状态：F0–F5 已实施并通过回归（`BOOT_TEST=1` drill 由 `tools/check_fault_handler.py` 固化；实现事实以源码为准，文末附实施记录与偏差）。

本文解决 [userboot 与 init 服务管理设计](service-manager.md) 阶段 C 暴露的 supervisor 死锁：init 同时是某个被监督服务的 client（同步 `Call`）和它的 fault receiver，一个 TCB 只有一个阻塞态，于是 fault 投递不进去、`Call` 也等不到回复。seL4 的答案是**把 fault handler 做成独立线程**；这要求本项目先支持"多个 TCB 共享 CSpace/VSpace"。相关文档：[对象内存所有权模型](object-ownership.md)、[seL4 风格 ABI](sel4-abi.md)、[能力系统与 IPC 演进规划](evolution-plan.md)、[磁盘与 FAT32 用户态驱动设计](disk-driver.md)。

## 1. 问题回顾

`docs/service-manager.md` §9 的 drill 复现：init 在 READY handler 里 `ipc::call(console_ep, WRITE, [CRASH_MAGIC])`，把自己置为 `TASK_BLOCKED_REPLY`；console 处理该调用时触发 fault；内核 `send_fault` 试图把 fault 投递到 init 的 `control_ep`，但 init 当前不是 `TASK_BLOCKED_RECV`，fault 只能被 enqueue；console 随即阻塞在 fault send；运行队列为空 → idle。destroy 路径根本没执行到。

根因：**一个 TCB 只有一个阻塞态。** supervisor 不能既阻塞在同步 `Call` 上、又在 `Recv` 上等 fault。

seL4 的 fault 投递机制与本项目相同：`sendFaultIPC` → `sendIPC`，没有接收者就把故障线程 enqueue 阻塞（`kernel/src/kernel/faulthandler.c`）。seL4 没有在核心里消除这个环，而是用架构拆掉它：**fault handler 是独立线程**（`libsel4utils` 的 `sel4utils_start_fault_handler` / `fault_handler` 循环就是 `seL4_Recv(fault_ep)`），client 才阻塞在 `Call`。

## 2. 目标与非目标

目标：

- 支持多个 TCB 共享同一个 CSpace 与 VSpace（线程组）。
- 提供用户态 `ThreadGroup` 与 `FaultHandler`：监督者可以起一个只做 `Recv` 的线程，主线程可自由阻塞在 `Call`。
- 把 init 改造成"fault-handler 线程 + 主线程"，drill 通过。
- 固化不变式：**supervisor 的 fault-receiving 线程不得对它所监督的服务做阻塞 `Call`。**

范围外：SMP、优先级、MCS、共享 CSpace 的原子 CSpace 操作、跨线程 TLS。

## 3. 线程模型：TCB / VSpace / CSpace 分离

当前每个 `Task` 私有 `cspace` 与 `vspace`；`retype(Tcb)` 会顺带创建一个 `AddressSpace`。目标是把三者拆开：

```text
Tcb（线程：调度态、Execution/内核栈、寄存器、IPC 阻塞态）
  ├─ cspace: ObjectId   → CNode   （可被多个 Tcb 共享）
  ├─ vspace: ObjectId?  → VSpace  （可被多个 Tcb 共享）
  ├─ ipc_buffer: (va, FrameRef)
  └─ fault_ep: u64      （在 cspace 中解析，per-Tcb）
```

| 概念 | seL4 | 本项目目标 |
| --- | --- | --- |
| 线程 | TCB 对象 | `Object::Tcb` + 调度器槽 |
| 地址空间 | VSpace 对象，多个 TCB 可引用 | `Object::VSpace`，引用计数 |
| CSpace | CNode 对象，多个 TCB 可引用 | `Object::CNode`，引用计数 |
| 线程组 | 共享 CSpace/VSpace 的一组 TCB | 同义的多个 `Object::Tcb` |
| fault handler | 每 TCB 一个 fault ep，独立线程接收 | 同 |

### 3.1 对象改动

- `Object::Tcb(task_id)` 保持轻量标识，但 TCB 元数据（cspace/vspace/ipc_buffer/fault_ep）从调度器 `Task` 移到对象或 TCB 记录，并允许共享。
- `VSpace` 的 `owner: u64` 改为引用计数（或"被哪些 TCB 引用"的集合）；不再是"一个任务独占"。
- `CNode` 已经是独立对象；去掉"一个任务一个 cspace"的隐含假设。
- `task_spaces: BTreeMap<task, ObjectId>` 删除，改由 TCB 记录 `cspace`。

### 3.2 调度器改动

- `Task` / `Tcb` 持有 `cspace: ObjectId`、`vspace: Option<ObjectId>`（两者都可与别的 TCB 共享）、`ipc_buffer`、`fault_ep`。
- `current_root()` 从 TCB 的 vspace 取根；同组线程共享根，切换不刷 TTBR0。
- `retire_task` 改为 `retire_thread`：只回收线程自身的 `Execution`/内核栈；CSpace/VSpace 在还有 TCB 或 cap 引用时保留。
- `caller`/`blocked` 等 IPC 状态本就是 per-TCB，保持。

### 3.3 回收

`collect` 的根集合增加"TCB 引用的 cspace/vspace"：

- CNode 存活 ⇔ 被某个 TCB 引用，或被某个 cap 引用，或是某 TCB 的根 CSpace；
- VSpace 存活 ⇔ 被某个 TCB 引用，或被某个 cap 引用；
- 删除一个 TCB 不再直接删除其 CSpace/VSpace；最后一个引用消失后由 `collect` 回收。

## 4. ABI 改动

seL4 non-MCS 已有对应调用，本项目的对象方法表里也已预留标签：

| 标签 | 方法 | 现状 | 目标 |
| --- | --- | --- | --- |
| 5 | TCB_Configure | 已实现（单一 vspace 归属检查） | 允许同一 CSpace/VSpace 配置给多个 TCB |
| 9 | TCB_SetIPCBuffer | 未实现 | 设置 per-TCB IPC buffer |
| 10 | TCB_SetSpace | 未实现 | 单独更新 cspace/vspace |
| 11 / 12 | TCB_Suspend / Resume | 已实现 | per-TCB，保持 |
| 3 | TCB_WriteRegisters | 仅初始启动 | 用于新线程的 PC/SP |

创建"同组线程"的标准路径（不依赖内核 Runtime）：

```text
Untyped_Retype(Tcb, slot)                        # 新 TCB，无 space
CNode_Copy 共享 CNode / VSpace cap 到该 TCB 可用处
TCB_SetSpace(cspace_cap, vspace_cap)             # 或 TCB_Configure
TCB_SetIPCBuffer(ipc_frame_cap, va)
TCB_WriteRegisters(resume=1, pc=<entry>, sp=<stack in shared vspace>, ...)
```

其中 `vspace_cap` 与监督者主线程使用的是同一个 `Object::VSpace`，`cspace_cap` 是同一个 `Object::CNode`。

## 5. 用户态：ThreadGroup 与 FaultHandler

新增 `projects/libs/server` 能力（或 `projects/libs/user/src/thread.rs`）：

```rust
/// 在一个已有线程组里创建线程：共享 cspace/vspace，独立栈与 IPC buffer。
pub unsafe fn spawn_thread(
    group: &ThreadGroup,     // 共享的 CNode/VSpace/预算
    entry: usize,
    stack: usize,
    argument: u64,
) -> Result<Thread, Error>;

/// 专用 fault-handler：只 Recv 监督端点，收到 fault 后清理并通知主线程。
pub fn run_fault_handler(service: &Service, table: &mut ServiceTable) -> !;
```

`ThreadGroup` 持有：共享 CNode cap、共享 VSpace cap、用于新线程的 untyped 预算、loader scratch。`spawn_thread` 负责：

1. `retype(Tcb)`（从组预算）；
2. 在共享 VSpace 里分配线程栈（retype SmallPage + map）；
3. `TCB_SetSpace` / `TCB_SetIPCBuffer`；
4. `TCB_WriteRegisters` + `TCB_Resume`。

`FaultHandler` 的形态（对应 `libsel4utils` 的 `fault_handler`）：

```rust
loop {
    let fault = ipc::recv(control_ep);        // 只 Recv，从不 Call 服务
    let index = fault.badge;                  // 服务编号
    reap_and_signal(table, index, fault);     // suspend/revoke 故障 TCB
}
```

## 6. init 的新结构

init 变成**一个共享 CSpace/VSpace 的线程组**：

```text
init 线程组（共享 CNode + VSpace + 预算）
  ├─ supervisor 线程   Recv(control_ep)：READY/REPORT/EXIT/Fault
  │                    spawn/restart 服务；只做内核对象调用（非阻塞）
  └─ client/logger 线程 需要时对 console/fs/block 做同步 Call（可阻塞）
```

分工原则：

- **supervisor 线程**：唯一在 `control_ep` 上 `Recv` 的执行体；只做非阻塞的内核对象调用（`Untyped_Retype`、`TCB_*`、`CNode_*`、`Runtime::Destroy` 等，均即时回复）。日志走**异步** `NBSend`（best-effort），或投递给 logger 线程。
- **client/logger 线程**：代表 init 对服务做同步 `Call`（例如 console 写）。它可能因服务崩溃而阻塞，但 supervisor 线程是**另一个 TCB**，能收到 fault、reap 掉故障服务，从而让 client 的 `Call` 以错误返回。

这样 §1 的死锁被彻底拆开：阻塞在 `Call` 的是 client 线程，收 fault 的是 supervisor 线程，二者不再是同一个阻塞态。

### 6.1 谁拥有服务表

- 服务表（cfg、状态、重启计数、TCB cap）由 supervisor 线程拥有；client 线程不直接改它。
- 同组线程通过**共享内存**（同一 VSpace）交换只读信息；可变状态由 supervisor 独占，client 通过消息请求。避免两个线程无锁地改同一张表。
- 若 client 需要发起重启（不常见），通过 endpoint 向 supervisor 发消息，而不是直接操作表。

## 7. 不变式

必须写进代码注释与文档，并用测试守住：

1. **fault-receiving 线程不得对它所监督的服务做阻塞 `Call`。** 需要请求/响应的调用由另一个线程发起。
2. 同组线程共享 CSpace：cap 的可见性是组级的；跨线程的 CSpace 变更由用户态负责同步（内核单核、IRQ 屏蔽，无数据竞争，但有逻辑竞争）。
3. 每个 TCB 仍有独立内核栈、独立 IPC buffer、独立 fault ep。
4. TCB 退出只回收自身；组资源在最后一个 TCB/cap 引用消失后回收。
5. **组内线程也必须有 fault ep，由 supervisor 监督**（[进程/线程组生命周期与组内故障监督](thread-group.md) §4）：fault handler 自己不能无声死亡。

## 8. 生命周期与回收

- `retire_thread(tcb)`：丢弃 `Execution`/内核栈；把该 TCB 从等待队列/调用关系中移除；唤醒其 caller（`Some(0)` 使其 `Call` 失败）。
- CSpace/VSpace 引用：TCB 被删除时递减引用；到零且无 cap 时由 `collect` 回收。
- 预算：同组共享一块子 untyped；整组销毁时 `Revoke` 该子 untyped。线程退出不单独回退 watermark（与现有 Untyped 语义一致）。
- `collect` 的根：所有 cap + 所有 TCB 引用的 cspace/vspace + 活跃 TCB 的 VSpace。

## 9. 分阶段实施

| 阶段 | 内容 | 前置 |
| --- | --- | --- |
| F0 | 内核：TCB/VSpace/CSpace 分离；`TCB_SetSpace`/`SetIPCBuffer`；多 TCB 共享 vspace；回收更新 | 现有对象模型 |
| F1 | 用户态 `ThreadGroup::spawn_thread`：同组建线程、共享 CSpace/VSpace、独立栈 | F0 |
| F2 | `rstiny-server` `FaultHandler`：`Recv` 循环 + reap + 通知/重启 | F1 |
| F3 | 改造 init：supervisor 线程 + client/logger 线程；fault 走 supervisor | F2 |
| F4 | 重跑 `BOOT_TEST=1` drill：崩溃 → supervisor reap → 重启 → console 恢复 | F3 |
| F5 | 文档与不变式固化；清理"supervisor 同步 Call"路径 | F4 |

## 10. 测试与验收

- 同组两个线程共享 CSpace/VSpace：一个线程插入的 cap，另一个线程可解析；共享的 VSpace 映射双方可见。
- 独立内核栈与 IPC buffer：两线程各自阻塞/恢复不互相覆盖。
- **drill**：`BOOT_TEST=1`；client 线程 `Call(console)`，console fault；supervisor 线程收到 fault、reap、重启；client 的 `Call` 以错误返回而不是永久阻塞；console 恢复输出。
- TCB 退出只回收自身；最后一个 TCB 退出后 CSpace/VSpace/预算被回收（`available` 回基线）。
- 负向：`TCB_SetSpace` 用无效 cap、跨组共享、重复配置；`TCB_Suspend`/`Resume` 对已终结线程。
- 权限：新线程拿不到组外 cap；fault handler 只能通过共享 CSpace 里的 cap 行事。

## 11. 与现有设计的关系

| 文档 | 需要修订 |
| --- | --- |
| [service-manager.md](service-manager.md) §3.2 | init 从单线程改为线程组（supervisor + client/logger） |
| [service-manager.md](service-manager.md) §9 | STOP/restart 由 supervisor 线程执行；drill 走新结构 |
| [object-ownership.md](object-ownership.md) | TCB 从"调度器槽"升级为可共享 CSpace/VSpace 的线程对象 |
| [sel4-abi.md](sel4-abi.md) | `TCB_SetSpace`/`SetIPCBuffer` 从"保留标签"变为已实现；多 TCB 共享 vspace |
| [evolution-plan.md](evolution-plan.md) 阶段 2 | "TCB/VSpace 分离"与本文合并 |

## 12. 开放决策

组生命周期（单线程销毁 vs 组级销毁）与“fault handler 自身也要被监督”两个后续设计见 [进程/线程组生命周期与组内故障监督](thread-group.md)。

1. 线程栈：从组预算 retype 固定大小（建议 16 KiB）vs 由调用者提供。
2. CSpace 共享的同步：约定"单写者"（supervisor 独占服务表）vs 引入用户态锁。
3. `TCB_SetSpace` 是否允许把一个已运行线程换到另一 VSpace（seL4 允许，但风险高）：v1 只允许未启动线程。
4. 是否保留 `Runtime` 托管层来建线程：目标是不依赖；迁移期可先用 `Runtime` 建"同组线程"作为过渡。
5. 线程命名/调试：是否给 TCB 加 name（影响对象布局），v1 先不做。

## 13. 实施记录（F0–F5）

已实施并通过 `make check`（含新增 `tools/check_fault_handler.py`，覆盖 debug/release × LOG=info/off 四种组合）。

**F0 内核（TCB/CSpace/VSpace 分离）**：

- `VSpace` 的单属主字段 `owner` 删除；`TcbConfigure` 不再做 `owner != 0 && owner != target` 独占检查——同一 CSpace/VSpace 可以配置给任意多个 TCB（`kernel/src/object/invoke.rs`）。
- `Store::task_spaces` 映射删除；调度器 `Task` 的 `cspace`/`vspace` 字段是唯一权威。线程终结（`finish`）调用 `retire_thread`（原 `retire_task`）：只回收线程的 `Execution`/内核栈与（托管任务的）IPC buffer cap，**不再清空共享 VSpace 的映射**；绑定字段保留到槽位复用/销毁，供销毁路径精确释放该线程自己的对象。
- `collect` 根集合 = 所有 cap ∪ 所有**非终态**线程引用的 cspace/vspace（`task::api::thread_roots()`，终态线程不再保持对象可达）∪ VSpace 传递引用的帧。组内最后一个 TCB/cap 引用消失后由按需标记-清除回收。
- 新增 ABI 方法（标签早已在 `projects/libs/abi` 预留）：`TCB_SetSpace`(10)（words: faultEP + cspaceData/vspaceData=0，caps: CNode、VSpace）与 `TCB_SetIPCBuffer`(9)（word: 1 KiB 对齐地址，cap: 已映射的 Frame；地址 0 = 清除）。两者与 `TcbConfigure` 一样只作用于未启动线程（§12.3 的 v1 决策）。
- `Runtime::Cspace/Vspace/Destroy`（托管过渡层）改由调度器任务字段解析目标空间；`release_task_objects` 在 VSpace 仍被其他存活线程引用时跳过删除（线程组安全）。
- TTBR0 无需改动：`UserContext::run` 本就按任务每次进入 EL0 装载根并恢复内核根，同根线程切换天然不重复刷 TLB；`AddressSpace::Drop` 的 TTBR0 断言依赖 collect 只在调度器栈上运行这一既有边界。

**F1 用户态线程组（`projects/libs/user/src/thread.rs`）**：

- `ThreadGroup::new(cnode, vspace, untyped, slot_base)` + `unsafe spawn_thread(entry, argument)`：从组预算 retype TCB；在共享 VSpace 的每线程 2 MiB 窗口（基址 0x0400_0000，最多 8 线程）retype L3 PageTable + 4 页 16 KiB 栈 + 1 页 IPC buffer 并映射；`Tcb::set_space`（共享 CSpace/VSpace，fault_ep=0）→ `Tcb::set_ipc_buffer` → `write_initial_registers(resume=true)`。失败逐 cap 回滚。`capability.rs` 新增两个包装。
- 独立内核栈、独立 fault ep 是内核既有性质（每 `Task` 一个 `Execution`；`fault_ep` 为 per-TCB 字段）；IPC buffer 由 `TPIDRRO_EL0` 按任务在每次进入 EL0 时装载。

**F2/F3 init 线程组（`projects/apps/init/{main,logger}.rs`）**：

- supervisor（主线程）：唯一 `control_ep` 接收者；只做即时内核对象调用（retype/Configure/Destroy/Revoke）与 `NBSend`；`ReplyRecv`/`Recv` 之外从不阻塞在服务上。
- client/logger 线程（`logger::run`）：init 内唯一对被监督服务做阻塞 `Call` 的执行体。supervisor 通过 `logger::LOG_EP` 投递日志请求（label `0x110`，payload 全部走消息寄存器——两线程不共享可变内存），`NBSend` 尽力而为，logger 忙时丢弃；supervisor 绝不等待 logger（否则 fault 可能落在无法接收的窗口）。
- drill（`BOOT_TEST=1`）：crash 写随 READY 日志一并投给 logger（`FLAG_CRASH_DRILL`），由 logger 对 console 发 `CRASH_MAGIC`。console fault → supervisor（阻塞在 Recv）收到 → reap（`Task::destroy` + 设备 untyped `Revoke`，见下）→ logger 的 Call 以内核错误 label 返回（`ipc::call` 以 `Ok(Received)` 承载回复，错误在 `label` 字段）→ backoff → 重启 → 新 console READY → 客户端日志再次走 console。验收证据行由 supervisor 在 `take_drill_result()`（共享内存标志，单写者，§6.1）之后统一打印，避免多线程 debug 输出交错。

**F4 回归与既有缺陷修复**：

- 新增 `tools/check_fault_handler.py`（§10 验收全项），纳入 `make check`。
- 修复服务重启的两个真实缺陷（drill 首次跑通时暴露）：
  1. **设备 untyped 不随服务回收**：console 的 UART 帧从共享设备区间切出，重启后区间 watermark 已耗尽（`NoMemory`）。`stop_and_reap` 现按服务授予的设备列表 `CNode_Revoke` init 侧设备 cap，重置子区间（seL4 `resetUntypedCap` 语义，service-manager.md §8）。
  2. **服务退出不清 `console_running`**：崩溃后日志仍尝试走 console，会阻塞投递者。现在服务终结时置 false，日志回落内核 debug console。

**与设计的偏差**（均已在正文对应小节标注）：

- TCB 元数据未从调度器 `Task` 迁入对象表：`Object::Tcb(task_id)` 与调度器槽 1:1 绑定，`Task` 字段即 TCB 记录；所有权语义（共享、按引用回收）不变。
- `VSpace` 未引入引用计数：可达性由 collect 的根集合（caps + 存活线程引用）计算，等价于"被哪些 TCB 引用"的集合，且省去增减计数的一致性维护。
- `run_fault_handler(service, table)` 未落入 `libs/server`：fault 接收就是 init supervisor 主循环（§6 的合并形态），服务表本就归 supervisor 线程私有；`libs/server` 维持服务侧运行时不变。

后续演进：组级销毁与组内线程的 fault 监督（本文 §3.3/§7/§8 的延伸）见 [进程/线程组生命周期与组内故障监督](thread-group.md)，已实施。
