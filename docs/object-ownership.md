# 对象内存所有权模型

日期：2026-09-10。状态：已实施。本文记录 `kernel/src/object/` 的单所有者模型，以及它与 Revoke、Untyped 计费、设备内存和对象回收顺序的关系。实现事实以源码为准；ABI 细节见 [seL4 ABI](sel4-abi.md)。

## 1. 问题：帧的三个所有者

旧实现中同一个物理帧的生命周期由三处 `Rc` 计数共同决定：

- `Store.objects` 的 `Object::Page(Rc<Frame>)`；
- CSpace 中引用该对象的 capability；
- `AddressSpace` 内部为页表项保存的 `Rc<Frame>`。

`collect` 只能"尽力回收"：它不知道地址空间还持有多少帧，也无法保证映射与对象同时消失。Revoke、Untyped 计费、设备内存和对象回收顺序都建立在这个所有权之上，所以必须先把它收敛为单一所有者。

## 2. 方向：对象表单一所有者

```text
ObjectTable (kernel/src/object/id.rs)
  slot[index] = { generation, object: Option<Object> }
        │
        ├── Object::Frame(Frame)        ← 唯一的物理页所有者
        ├── Object::PageTable(Frame)
        ├── Object::VSpace(VSpace)      ← 持有 FrameRef，不持有 Frame
        ├── Object::CNode(CNode)        ← 持有 capability 槽
        ├── Object::Tcb(u64)            ← 调度器任务槽
        ├── Object::Untyped
        ├── Object::AsidPool
        └── Object::Runtime
```

- 对象表是**唯一**的 payload 所有者。capability、地址空间和任务只保存 `ObjectId`。
- `Frame` 的 `Drop` 仍然把物理页归还帧池，但只有对象表移除该条目时才会触发。
- `AddressSpace` 只保存 `FrameRef { id, virt, physical }`：身份用于回收，地址用于安装页表项。物理地址不可变，因此无需在操作时回查对象表。
- 对象槽复用时 `generation` 递增，旧 `ObjectId` 解析为 `None`，杜绝 ABA。

`ObjectId` 不跨用户 ABI。用户只传 CSpace 槽号，内核在调用者 CNode 中解析。

## 3. 生命周期

### 3.1 引用来源

对象表本身不知道谁"需要"一个对象。可达性由两类引用决定：

| 来源 | 形式 | 说明 |
| --- | --- | --- |
| capability | `Cap.object: ObjectId` | CNode 槽内，撤销时整体消失 |
| 任务绑定 | `Task.vspace` / `task_spaces` | 调度器持有的 CSpace 与 VSpace 根 |
| 地址空间映射 | `VSpace.space.frame_refs()` | 映射页与页表帧 |

`collect` 从 capability、`task_spaces` 和 `api::vspace_roots()` 出发做标记，再沿 `VSpace → FrameRef` 展开，最后删除不可达对象。删除 `VSpace` 会丢弃它的 `FrameRef` 列表，下一轮扫描即可回收这些帧。

### 3.2 映射与 capability 绑定

`Cap.mapping: Option<Mapping { space, address, table }>` 记录该 capability 建立的映射。映射与 capability 同生共死：

- `ARM_Page_Map` / `ARM_PageTable_Map` 建立映射并写入 `mapping`；
- `ARM_Page_Unmap` / `CNode_Delete` / `CNode_Revoke` 先解除映射，再移除 capability；
- 解除映射只把 `FrameRef` 从 `AddressSpace` 移除，帧对象仍由对象表持有，直到没有 capability 或地址空间引用它。

因此**一个帧被映射 ⇔ 至少有一个 capability 记录着该映射**，帧的存活不需要独立的反向映射数据库。

### 3.3 回收：按需 collect

`collect` 是有界的标记-清除，成本与对象表大小相关，因此不能放在每个 syscall 的快速路径上。回收改为按需触发：

```text
释放引用的操作（delete/revoke/unmap/retype 回滚/task 退出）
        │ request_collect()
        ▼
COLLECT_PENDING = true
        │ 下一个 IRQ-masked 安全边界
        ▼
collect_if_requested() → collect()
```

安全边界有两处：

- `object::call` 在整次对象调用返回后；
- `task::scheduler::run` 在 `complete_run` 释放调度器借用后（任务退出路径）。

这样多步创建（Retype、Create）在发布期间不会被误回收，而释放后的帧会在下一次边界被回收。

### 3.4 任务退出

- `retire_task`：托管任务退出时移除 IPC capability，并清空其 VSpace 的 `space`（丢弃全部 `FrameRef`）。
- `forget_task`：移除托管 CSpace。
- `release_task_objects`：显式运行时策略销毁标准 TCB 时，移除 TCB 对象并清空指向它的 capability。
- `scheduler::finish` 把 `Task.vspace` 置空，使该地址空间不再是根。

帧本身由随后的 `collect` 回收，而不是依赖 `Drop` 链。

## 4. 与 seL4 的对应与差异

| 概念 | seL4 | 本项目 |
| --- | --- | --- |
| 对象身份 | 物理地址 / capPtr + MDB | 对象表 `ObjectId` + generation |
| 对象内存 | 从 Untyped 切分，对象表不存在 | 内核元数据对象表（有界 4096） |
| 撤销 | MDB 派生树 | `parents` 派生记录 + 有界扫描 |
| 帧映射追踪 | MDB | capability 上的 `mapping` 记录 |
| 对象回收 | 引用归零 + MDB 清理 | 按需标记-清除 |

保留的简化：

- Untyped 仍是帧池分配权限的抽象，没有物理区间 watermark 与精确对象布局；对象表本身不随用户请求增长。
- TCB 状态仍在调度器中，对象表只保存 `Object::Tcb(task_id)` 标识。
- 单核、IRQ-masked 假设不变，对象表没有锁。

## 5. 代码地图

```text
object/
  mod.rs      Store、Object、Cap、Mapping、生命周期与 collect
  id.rs       ObjectId、ObjectTable（generation + 空闲链）
  cnode.rs    CNode 槽操作、派生撤销、映射解除
  invoke.rs   对象调用分发、Retype、Map/Unmap、TcbConfigure
  runtime.rs  托管运行时扩展（Create/Destroy/Map/...）
memory/
  frame.rs    Frame（唯一物理页所有者）与 FrameRef（非拥有引用）
  space.rs    AddressSpace：只保存 FrameRef 的映射元数据
```

## 6. 后续

所有权模型稳定后，以下工作可以直接建立在其上：

具体的数据结构、BootInfo v5 布局、启动分区、retype/Revoke 流程、finalisation 清单与分阶段验收见 [Untyped 物理内存实现计划](untyped-plan.md)。

1. **Untyped 计费**：已实施。对象表槽记录 `ObjectOwner`，对象内存由 `Untyped` 的 watermark 切分，`Revoke` 重置区间并 finalise 子对象。
2. **设备内存**：已实施设备 Untyped（UART），设备 Untyped 只能生成设备 Frame；GIC/timer 保留给内核。
3. **回收顺序**：`collect` 仍是托管兜底，标准对象路径由 `Revoke/Delete` 完成；后续可在 `Untyped::reset` 中加入设备 DMA 静止检查。
4. **SMP**：把对象表的 `SingleCore` 封装替换为带锁的表，`ObjectId` 的 generation 语义保持不变。
