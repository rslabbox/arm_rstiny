# 进程/线程组生命周期与组内故障监督

日期：2026-09-13。状态：G0–G3 已实施并通过回归（drill 由 `tools/check_fault_handler.py` 固化；实现事实以源码为准，文末附实施记录与偏差；G4 仍为可选项）。

本文补上 [独立 fault-handler 线程与线程模型](fault-handler.md) F4 落地后暴露的两个缺口：

1. **线程组没有生命周期概念。** 显式销毁一个 TCB 会把它与兄弟线程共享的 CSpace 一起删掉（`forget_task` 无条件删除 CNode），兄弟线程随后持有悬空的 `ObjectId`。
2. **组内线程自己没有 fault endpoint。** `ThreadGroup::spawn_thread` 把 `fault_ep` 设为 0，logger 线程崩溃时无人知晓。

相关文档：[独立 fault-handler 线程与线程模型](fault-handler.md)、[userboot 与 init 服务管理设计](service-manager.md)、[对象内存所有权模型](object-ownership.md)、[seL4 风格 ABI](sel4-abi.md)。

## 1. 问题 1：共享 CSpace 被单线程销毁拖垮

`kernel/src/object/mod.rs` 当前：

```rust
pub(crate) fn forget_task(task: u64, cspace: Option<ObjectId>) {
    with_store(|store| {
        if store.managed.remove(&task) && let Some(cspace) = cspace {
            store.objects.remove(cspace);       // 无条件删除，不看兄弟线程
        }
    });
}
```

而 `release_task_objects(task, vspace)` 对 VSpace 做了兄弟检查：

```rust
let shared = vspace.is_some_and(|id| api::thread_roots().contains(&id));
```

于是 VSpace 被保留、CSpace 被删除，**两者不对称**。init 是 managed，且 supervisor 主线程与 logger 是同一 CSpace；`Runtime::Destroy(init)` 一旦发生（userboot 在 init 崩溃后重启它就会走这条路），logger 的 `task.cspace` 立刻悬空，之后每次 `ipc::recv` 都在 `resolve` 处拿到 `INVALID_CAPABILITY` 并空转成僵尸线程。

根因：**模型里没有"线程组"这个一等概念，销毁一个 TCB 时无法区分"单线程任务"与"组里的一个成员"。** 现有实现靠 `thread_roots()` 事后推断共享，只能在"删除对象"时补救，管不了"组里还有谁在跑"。

## 2. 设计 1：进程/线程组

### 2.1 组身份

采用"**CSpace 为组键**"：一个线程组 = 所有满足 `tcb.cspace == X` 的活跃 TCB。理由：

- CSpace 是权限边界，也是 `sel4utils_process` 里"进程"的实际含义；
- 与用户需求"销毁所有引用同一 cspace/vspace 的 TCB"直接对应；
- 不需要新增内核对象类型（见 §2.5 的可选 Process 对象）。

组内成员约定共享同一个 VSpace；若出现"同 CSpace、不同 VSpace"的成员，按本设计视作非法/未定义，销毁时按 CSpace 集合处理，各自 VSpace 由引用回收。

### 2.2 销毁语义

当前 `Runtime::Destroy` 只销毁一个 TCB。改为：

| 调用 | 语义 |
| --- | --- |
| `Runtime::Destroy(handle)` | **组级**：销毁所有引用该 TCB 的 CSpace 的 TCB，然后回收 CSpace/VSpace/预算（对应"销毁进程"） |
| `Runtime::DestroyThread(handle)`（新增） | 单线程：只销毁这一个 TCB；共享 CSpace/VSpace 保留给兄弟线程 |

把 `Destroy` 定义为组级，是因为 `Runtime::Create` 返回的句柄语义上命名一个**进程**，而 `ThreadGroup::spawn_thread` 又在同一个进程里加线程。单线程销毁是少见路径，单独给一个标签，名字直白。

### 2.3 销毁顺序

```text
DestroyGroup(handle):
  target = resolve(handle)
  cspace = target.cspace
  members = { tcb : tcb 活跃且 tcb.cspace == cspace }

  1. 停止所有成员线程
       - 从就绪/睡眠/等待队列移除
       - 清 blocked / caller，并以错误唤醒外部等待其 reply 的 client
       - 丢弃 Execution / 内核栈
  2. 释放组资源（此时已无活跃线程引用）
       - 删除 managed CSpace（含其 INIT_IPC_BUFFER cap）
       - 回收每个成员引用的 VSpace（若无其它引用）
       - Revoke 组的子 untyped 预算；设备 untyped cap 显式 Revoke（沿用 stop_and_reap 的做法）
  3. request_collect()
       - VSpace 映射的帧由 mark-sweep 在最后一个引用消失后回收
```

顺序的关键：**必须先停掉所有成员，再删共享对象**；否则又回到"删了 CSpace、线程还在跑"的悬空状态。第 1 步"唤醒外部 client"也很重要：某个成员可能是别人 `Call` 的 server，销毁它要让阻塞的 client 以错误返回，而不是永久阻塞。

### 2.4 引用检查（对称化）

即使有了组级销毁，单线程路径也必须安全。把两处收尾统一成"引用检查后再删"：

```rust
// forget_task：仅当没有别的活跃线程引用该 CSpace 时才删除
if managed.remove(task) && let Some(cspace) = cspace {
    if !thread_roots().contains(&cspace) {
        store.objects.remove(cspace);
    }
}
```

`release_task_objects` 的 VSpace 检查保持不变。这样：单线程销毁保留共享对象，组级销毁在停掉全部成员后自然删除——两条路径都正确。

更进一步的做法是**根本不在销毁路径删对象**：只清线程绑定、`request_collect()`，让 mark-sweep 统一回收。但 managed 层需要"进程销毁即回收 CSpace"的确定性，因此保留"引用检查后的显式删除"。

### 2.5 可选：一等 `Process` 对象

若希望组有稳定的能力身份和显式的账本，可加一个内核对象：

```rust
struct Process {
    cspace: ObjectId,
    vspace: ObjectId,
    budget: Option<ObjectId>,     // 组的子 untyped
    threads: BTreeSet<TcbId>,     // 成员
}
```

- `Process_Create`：retype Process，绑定 CSpace/VSpace，切预算；
- `Process_AddThread` / `RemoveThread`：把 TCB 加入/移出组（设置其 cspace/vspace）；
- `Process_Destroy`：= §2.3；
- collect：Process 是根，root 其 cspace/vspace/budget 与成员 TCB；TCB 由 Process + cap 保活。

**建议**：v1 用"CSpace 为组键"（§2.1–2.4），把 `Process` 对象列为 G4 可选。理由：seL4 内核没有 Process 对象，"进程"是用户态抽象；引入它会增加内核对象种类，而当前最缺的是**销毁语义正确**，不是组身份的稳定性。等 appmgr 需要"按句柄管理一批进程"时再评估。

### 2.6 与 collect 的关系

- 组的 CSpace/VSpace 是 `collect` 的根吗？——不是直接根。根是"活跃 TCB 的 cspace/vspace"（`thread_roots`）和 cap。组级销毁会先让所有成员不再 rooted，然后显式删除；单线程销毁靠引用检查保留。
- TCB 本身由 cap 保活；销毁后 cap 被 `CNode_Revoke`/`Delete` 清掉，`collect` 回收其对象。
- 预算：组共享一块子 untyped，整组销毁时 `Revoke`；单线程退出不回退 watermark（与 Untyped 语义一致）。

## 3. 问题 2：组内线程崩溃无声

`ThreadGroup::spawn_at` 里：

```rust
unsafe { thread.set_space(self.cnode, self.vspace, 0)? };   // fault_ep = 0
```

`fault_ep == 0` 时，内核 `deliver_or_terminate` 找不到 fault endpoint，直接 `Disposition::Fault(...)` 终止该线程。于是 logger（以及任何用 `spawn_thread` 建的组内线程）崩溃时：

- supervisor 的 `Recv(control_ep)` 收不到任何消息；
- 组的日志通道静默废弃；
- 系统看起来还在运行。

这是"独立 fault handler"形态带出的对称问题：**fault handler 自己也需要被监督。**

## 4. 设计 2：组内故障监督

### 4.1 每个组内线程都有 fault ep

把 `spawn_thread` 扩展为接收 fault endpoint：

```rust
pub unsafe fn spawn_thread(
    &mut self,
    entry: usize,
    argument: u64,
    fault_slot: u64,     // 组共享 CSpace 里的 slot
    fault_badge: u64,    // 该线程的内部 badge
) -> Result<Thread, Error>;
```

调用方（supervisor）为每个内部线程 mint 一个 `control_ep` 的 badged cap，放进共享 CSpace 的 `fault_slot`，再把它作为 `fault_ep` 传给 TCB。

### 4.2 badge 空间

沿用 [service-manager.md](service-manager.md) §10 的"一个 `control_ep` + badge"：

| badge 范围 | 含义 |
| --- | --- |
| `1 ..= 0x7FFF` | 服务（按配置顺序） |
| `0x8000 + index` | 组内线程（logger 等） |

supervisor 的 `Recv` 按 badge 区分：服务 fault/READY/REPORT 走服务生命周期；内部线程 fault 走"重建该线程"。

### 4.3 supervisor 处理内部 fault

```text
on Recv(control_ep):
  badge < INTERNAL_BASE  -> 现有服务逻辑
  badge >= INTERNAL_BASE -> 内部线程 index
      reap 该 TCB（它已因 fault 阻塞）
      重建：spawn_thread(entry, arg, fault_slot, badge)
      记录/（尽力）日志
```

约束不变（[fault-handler.md](fault-handler.md) §7）：**supervisor 仍然只 `Recv`，不对被监督对象做阻塞 `Call`**。重建 logger 用的是非阻塞内核对象调用，不破坏不变式。

### 4.4 谁监督 supervisor

supervisor 主线程的 fault ep 由它的父级设置：init 由 userboot 经 `spawn_supervised` 创建，fault ep 指向 userboot 的 `control_ep`（badge = init）。所以：

| 线程 | fault ep 指向 | 监督者 |
| --- | --- | --- |
| userboot 主线程（root task） | 无（最高级） | 无（park/panic） |
| init supervisor | userboot 的 control_ep | userboot（level-1 重启） |
| init logger 等组内线程 | init 的 control_ep（内部 badge） | init supervisor |
| console 等服务 | init 的 control_ep（服务 badge） | init supervisor |

形成一条链：**userboot → init supervisor → 组内线程/服务**。每层只 `Recv` 自己的监督端点，不做阻塞 `Call`。

### 4.5 边界

- 内部线程 fault 时，supervisor 只重建线程本身；不动组内其它线程与 CSpace/VSpace。
- 若重建连续失败（资源耗尽等），supervisor 记录并停止重建该内部线程（可复用 restart storm guard 的思路）。
- 组内线程的 fault 消息 label 仍是 fault label（0..=4），badge 用于区分来源；与服务的 fault 共用一条投递路径，不新增内核机制。

## 5. 与现有设计的关系

| 文档 | 需要修订 |
| --- | --- |
| [fault-handler.md](fault-handler.md) §3.3/§8 | 退役改为组感知：单线程退役做引用检查，组级销毁先停成员 |
| [fault-handler.md](fault-handler.md) §7 | 不变式补一条"组内线程也必须有 fault ep，由 supervisor 监督" |
| [service-manager.md](service-manager.md) §3.2/§10 | init 的 supervisor 处理内部 badge；重启策略对内部线程同样适用 |
| [sel4-abi.md](sel4-abi.md) | `Runtime::Destroy` 变组级；新增 `Runtime::DestroyThread` |
| [object-ownership.md](object-ownership.md) | 若采纳 §2.5，登记 `Process` 对象；否则维持 CSpace 为组键 |

## 6. 分阶段实施

| 阶段 | 内容 | 说明 |
| --- | --- | --- |
| G0 | `forget_task` 引用检查；`release_task_objects` 对称化 | 最小改动，先堵悬空 |
| G1 | `Runtime::Destroy` 组级 + `Runtime::DestroyThread`；userland `ThreadGroup::destroy` 先停成员再收资源 | 组生命周期 |
| G2 | `spawn_thread` 接 fault ep；内部 badge 空间；supervisor 处理内部 fault 并重建 logger | 组内故障监督 |
| G3 | 测试：单线程销毁保留兄弟；组销毁回收全部；logger 崩溃被重建；init 崩溃被 userboot 重启且旧组完全回收 | 见 §7 |
| G4（可选） | 一等 `Process` 对象 | 需要稳定组身份时再做 |

## 7. 测试与验收

- **G0**：两线程组，销毁一个 TCB，兄弟线程继续 `Recv`/`Call` 正常，无 `INVALID_CAPABILITY`；CSpace 未被删。
- **G1**：销毁整组后，所有成员线程消失，CSpace/VSpace/子 untyped 回收，`Runtime::AvailableFrames` 回基线。
- **G1 负向**：销毁正在被外部 `Call` 的成员，外部 `Call` 以错误返回而不是永久阻塞。
- **G2**：让 logger 线程故意触发 fault（测试构建的 drill 开关），supervisor 收到内部 badge、重建 logger，其后日志恢复。
- **G2 链式**：让 init supervisor 触发 fault，userboot 重启 init；旧组的 supervisor+logger 都必须被回收（无僵尸线程、无悬空 CSpace、帧回基线）。
- **回归**：`tools/check_fault_handler.py`、`check_userboot.py`、`check_ipc.py`、`check_untyped.py`、`check_capabilities.py` 全绿；建议新增 `tools/check_thread_group.py`。

## 8. 开放决策

1. 组键：CSpace（本设计建议）vs 新增 `Process` 对象（§2.5）。
2. `Runtime::Destroy` 是否直接改语义为组级，还是保留单线程并只加 `DestroyGroup`。本设计建议前者 + `DestroyThread`。
3. 组内线程 fault 是否复用 `control_ep` + 内部 badge（建议），还是另开 `internal_ep`。
4. 组内线程重建策略：立即重建（建议）vs 退避 + 上限。
5. "同 CSpace、不同 VSpace"的成员是否允许：本设计视为未定义，销毁按 CSpace 集合处理。

## 9. 实施记录（G0–G3）

已实施并通过 `make check`（含扩展后的 `tools/check_fault_handler.py`，debug/release × LOG=info/off 四种组合）。

**G0 引用检查对称化**：`forget_task` 新增 `shared` 参数——由调用方（`task::api::destroy`，已持有 scheduler 借用，故不能内部再调 `thread_roots()`，`SingleCore` 不可重入）在清槽前扫描同 CSpace 的其他存活线程；仅当无兄弟引用时才删除 managed CSpace。`release_task_objects` 的 VSpace 检查保持不变，两条收尾路径对称。

**G1 组级销毁**：

- `task::api::group_members(target)`：返回共享 `target` CSpace 的全部线程 id（含终态成员，销毁时一并清槽）。
- `Runtime::Destroy(handle)` 改为**组级**：先逐成员 `api::destroy` + `release_task_objects`（共享 VSpace 由兄弟检查保留，最后一个成员的释放将其回收），再返回；`members` 含当前线程时报 `INVALID_ARGUMENT`。
- 新增 `Runtime::DestroyThread = 0x1013`：单线程销毁，共享 CSpace/VSpace 留给兄弟。userland 对应 `Task::destroy_thread`（组级 `Task::destroy` 语义已在文档注明）与 `Task::from_tcb`。

**G2 组内故障监督**：

- `ThreadGroup::spawn_thread` 增加 `fault: Option<FaultSupervision>`：supervisor 提供未 badge 的 control cap 槽号 + 落点槽 + badge，spawn 内部 `CNode_Mint` 出该线程的 fault cap 并写入 `TCB_SetSpace` 的 fault_ep。deviation：文档 §4.1 签名中的 `fault_badge` 参数省去——badge 烙在 cap 里，内核投递时从 cap 读取，无需单独传。
- init：logger 的 fault cap 由 `CONTROL_OBJ` mint（badge = `0x8000`，slot 144）；supervisor 主循环在服务 badge 匹配前处理 `badge >= 0x8000` 的内部 fault——`Task::destroy_thread` 收割该 TCB → `Thread::release`（新方法，删除该线程的 page-table/栈/IPC buffer cap，避免重建泄漏）与 fault cap → 重新 `spawn_thread` → 通过**可靠 Send**（仅对刚重建、无在途 Call 的 logger 安全）投一条验证日志。
- logger 增加 `FLAG_CRASH_SELF`（读空指针自崩），供 drill 触发整条监督链。

**G3 验收（drill 与 userboot 配套改动）**：

- drill 序列（`BOOT_TEST=1`，仅第一代 init 生效——userboot 经 `SpawnInfo::extra[5]` 传递重启代数）：console 崩溃→supervisor reap→重启；logger 自崩→内部 badge→reap→重建→日志恢复（`logger rebuilt` 由重建后的 logger 经 console 写出）；随后 init 以 `EXIT(7)` 交回 userboot——**组级销毁**杀掉 supervisor+logger 全组，userboot 重建 init（generation=1，drill 不再运行），新 console 正常上电并被监督。
- userboot 配套修复：重启 init 前对其设备 untyped 副本（`UART_DEV_COPY`）执行 `CNode_Revoke`——同 init 预算一样，设备派生不随组销毁自动复位，否则重启后的 console 切不出 UART 帧（与 service-manager.md §24 补充记录的 stop_and_reap 修复同因）。
- 验收脚本未新增 `check_thread_group.py`：G0/G1/G2 全部由 init 真实生命周期 drill 覆盖（单线程销毁保留兄弟、组销毁回收全组、内部 fault 重建、level-1 重启后无僵尸），既有 managed 回归（check_tasks 等）守住单线程销毁路径。`Runtime::AvailableFrames` 回基线由重建成功隐式断言（预算区间未复位则新 init 必然 NoMemory）。

**文档事实修正**：§1 所述 "init 是 managed" 不准确——init 由 userboot 经标准对象操作（`spawn_supervised`）创建，不在 `managed` 集合，`forget_task` 的旧代码对它本就不会删 CSpace；但 userboot 的 `Task::destroy(init)` 走 `Runtime::Destroy`，悬空风险经 G1 的组级销毁消除，修复方向不变。
