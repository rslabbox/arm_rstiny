# 设备 IRQ 授权与用户态投递

日期：2026-09-14。状态：已实施（2026-09-10，I0–I4 全部落地，见 §13 实施记录）。

本文实现 [完整微内核设计与分阶段路线](microkernel-design.md) §14 的"设备中断与服务拆分"里尚未完成的一半：把设备中断从内核轮询/屏蔽改成 **seL4 风格的 `IRQControl`/`IRQHandler`/Notification 授权与投递**，让用户态驱动用通知等待完成而非轮询。服务拆分本身已在阶段 D 完成（block-server/fs-server/appmgr 独立进程），本文只补中断路径。

现状见 [中断控制器、定时器与调度 tick](interrupts.md)、[磁盘与 FAT32 用户态驱动设计](disk-driver.md)、[userboot 与 init 服务管理设计](service-manager.md)。

## 1. 现状与缺口

- 内核 GIC 只使能 timer PPI；其他中断在 `interrupt.rs` 的默认分支被直接屏蔽，以防风暴（`arch/machine/gic.rs`、`interrupt.rs`）。
- 没有 `IRQControl`/`IRQHandler` 对象，也没有 `IRQIssueIRQHandler`/`IRQAckIRQ`/`IRQSetIRQHandler`/`IRQClearIRQHandler`。
- 没有 `TCB_BindNotification`。
- `block-server` 是**轮询**：请求循环里 `device.read_blocks(...)` 同步完成，不等待设备完成中断。
- GIC 使用 combined EOI（`EOImode=0`，`complete()` 同时降优先级和 deactivate）。引入用户 IRQ 后需要明确 mask / EOI / Ack 的顺序。
- 设备 Untyped 只发布 UART 与 VirtIO MMIO；**GIC 不发布给用户态**，所以驱动不能自己操作中断控制器——这正需要内核提供 IRQ 授权机制。

## 2. seL4 的 IRQ 模型（目标语义）

| 概念 | seL4 | 说明 |
| --- | --- | --- |
| `IRQControl` | 全局单例 cap（`seL4_CapIRQControl`），root task 持有 | 用它为具体中断线派生 `IRQHandler` |
| `IRQHandler` | 指向某条中断线的 cap | `SetNotification` / `Clear` / `Ack` |
| `IRQControl_Get` | `irq, root, index, depth` | 在目标 CNode 生成 `IRQHandler` |
| `IRQHandler_SetNotification` | `irq_handler, notification` | 绑定 Notification，中断到来时 signal |
| `IRQHandler_Ack` | `irq_handler` | GICv3：`deactivateInterrupt`（写 `ICC_DIR_EL1`）；GICv2：`maskInterrupt(false)`。此后可再次投递 |
| `IRQHandler_Clear` | `irq_handler` | 解除绑定并把线置为 inactive（disable） |
| 投递 | 内核读 IAR（pending→active）→ 找到 handler → `Signal(ntfn, badge)` → 优先级下降（EOIR），**不 deactivate** | 中断保持 active 直到驱动 `Ack`（active 即隐式 mask） |

关键规则：

1. **GICv3 用户中断不显式 mask，靠 active 状态隐式屏蔽。** `getActiveIRQ` 读 IAR 后中断进入 active；投递只做优先级下降（EOIR），不 deactivate；驱动 `Ack` 才 deactivate（DIR），此后同一线才能再次投递。GICv2 才是显式 mask/unmask。seL4 的 `handleInterrupt` 只在 `!CONFIG_ARM_GIC_V3_SUPPORT` 时才 `maskInterrupt(true, irq)`。
2. **`badge` 取绑定的 Notification cap 的 badge**（root 通常把它 mint 成中断号），一个 Notification 可服务多条线，驱动按 badge 区分。
3. **`IRQControl` 只给 root**；普通任务只能拿到被显式授予的 `IRQHandler`/Notification cap。
4. **定时器与内核专用中断不可被用户 `Get`**；重复 `Get` 一条已 active 的线返回 `seL4_RevokeFirst`。
5. 驱动的 Notification 与 GIC MMIO 分离：驱动完全没有 GIC 映射，只能通过 `Ack` 请求内核 deactivate 线路。

## 3. 内核对象模型

新增两个对象（`kernel/src/object/mod.rs`）：

```rust
Object::IrqControl                    // 单例，root 持有
Object::IrqHandler(IrqHandler)
struct IrqHandler {
    irq: u32,                          // INTID（SPI/PPI/SGI）
    trigger: Trigger,                  // 由平台表给定，不由用户选择
    bound: Option<ObjectId>,           // 绑定的 Notification
}
```

- `IrqControl` 与 `Runtime` 一样是内核预置对象；`init_root` 把它放进固定槽位 `INIT_IRQ_CONTROL`（`projects/libs/abi/src/object.rs`，seL4 约定值 4）。
- `IrqHandler` 不是 `Untyped_Retype` 的类型，只能由 `IRQControl_Get` 创建（与 seL4 一致）。
- 对象表容量：每条被授权的线一个对象，数量有界（平台 SPI 数 + PPI/SGI）。

### 3.1 平台中断表

中断的触发方式（level/edge）与是否可授权由**平台表**决定，不允许用户指定。落地形式是**生成的一条一记录**：`build_platform.py` 从 DTB 提取每个可授权设备的 `interrupts` 属性，导出 `IRQ_LINES: &[(intid, level, kind)]`（kernel `config.rs`），并经 BootInfo 记录 `BOOTINFO_HEADER_IRQS=9`（`IrqDesc`）发布给 root。顺序即契约：**VirtIO 槽位线（kind 0）升序在前，其余设备线（kind 1，当前是 PL011 的 INTID 33）在后**；supervisor 因此能按位置把"第 N 个 VirtIO 设备"映射到线，无需解析 DTB。

```rust
// 生成（QEMU virt 实际值）：32 条 VirtIO 槽位线（边沿）+ 1 条 UART 线（电平）
pub const IRQ_LINES: &[(u64, u64, u64)] = &[(48, 0, 0), /* … */ (79, 0, 0), (33, 1, 1)];
```

`IRQControl_Get` 只接受表中列出的线；timer PPI 与未列入的线全部拒绝。授权策略是这张表本身，不是对窗口常量做算术。

## 4. ABI

`projects/libs/abi/src/object.rs::Invocation` 按 seL4 XML 顺序补齐（项目已实现子集，值不冲突）：

| 标签 | 方法 | 参数 |
| --- | --- | --- |
| 13 | `TcbBindNotification`（可选） | notification |
| 14 | `TcbUnbindNotification`（可选） | 无 |
| 26 | `IrqIssueIrqHandler` | `irq, root, index, depth`（目标 CNode） |
| 27 | `IrqAckIrq` | 无 |
| 28 | `IrqSetIrqHandler` | `notification` |
| 29 | `IrqClearIrqHandler` | 无 |

`INIT_IRQ_CONTROL = 4` 加入初始 cap 布局。错误码沿用现有集合：非法线/内核保留 → `InvalidArgument`；重复 `Get` 一条已 active 的线 → `RevokeFirst`（seL4 用这个名字）；目标槽非空 → `ALREADY_MAPPED`（wire 值 8，与 seL4 `seL4_DeleteFirst` 同值，用户库映射为 `Error::DeleteFirst`，语义一致）；错误对象类型 → `InvalidCapability`；无 `Write` 权 → `PermissionDenied`。`Get` 成功即 `setIRQState(IRQSignal)`，把线 **enable**（允许投递）。

用户态封装放在 `projects/libs/user/src/capability.rs`：`IrqControl::get`、`IrqHandler::{set_notification, ack, clear}`。设计中的 `server::wait_irq()` 便捷函数未实现——驱动对 Notification 直接 `ipc::recv` 已足够，不再加一层转发（§13.4）。

## 5. GIC 时序

seL4 的 GICv3 路径用 **split EOI**（`EOImode=1`）：优先级下降（`ICC_EOIR1_EL1`，EOIR）与 deactivate（`ICC_DIR_EL1`，DIR）是两次写。用户中断的“屏蔽”由 GIC 的 **active 状态**隐式提供，不是显式 mask：

```text
# 投递（内核 IRQ 入口）
claim()                       -> ActiveInterrupt(id)   # 读 ICC_IAR1_EL1：pending → active
if id == timer: tick.rearm(); priority_and_deactivate(id); reschedule
else if handler(id) bound:
    notification.signal(badge)              # 通知驱动
    priority_drop(id)                       # 写 EOIR1：仅降优先级
    # 不 deactivate、不 mask：中断保持 active，GIC 不会再投递这条线
else:                                        # 未知源 / 无 handler
    priority_and_deactivate(id)             # EOIR + DIR，立即回收
    set_enable(id, false)                   # 并屏蔽该线（seL4 IRQInactive）

# 驱动
读设备清源（VirtIO used ring + InterruptACK）
IRQAckIRQ(handler) -> deactivateInterrupt(id)   # 写 ICC_DIR_EL1
    # 此后源仍有效（电平未清）会再次投递——这是驱动未清源的信号，不是内核 bug
```

`IRQControl_Get`：`setIRQState(IRQSignal)` 把线 **enable**（`maskInterrupt(false)`），`Get` 成功就允许投递；重复 `Get` 一条已 active 的线返回 `RevokeFirst`。
`IRQHandler_Clear` / 删除 handler：`setIRQState(IRQInactive)` → `maskInterrupt(true)` **disable** 该线，避免无主中断风暴。

规则与边界：

- **用户中断不能 combined EOI**：combined（`EOImode=0`）会同时 deactivate，等于在驱动处理前就允许重新投递，电平源未清时会风暴。项目需把 `arch/machine/gic.rs` 从 `cpu.set_eoi_mode(false)` 改为 split（`true`），并暴露 `priority_drop`（EOIR）与 `deactivate`（DIR）两步；现有 timer/未知源仍走“EOIR+DIR 立即回收”。
- **电平中断**：驱动必须先清设备源再 `Ack`，否则立即再次触发。
- **边沿中断**：`Ack` 只回收 active 状态，无需“清源”。
- `claim` 返回的特殊/伪 ID 不投递、不 EOI。
- 定时器 PPI 与内核专用线不可 `Get`。

GICv2 对照：`getActiveIRQ` 读 IAR，投递后**显式** `maskInterrupt(true)`，`Ack` 用 `maskInterrupt(false)` 重新开放。本平台是 GICv3，走上面的 active/deactivate 模型。

## 6. Notification 交付

复用现有 `Object::Notification` 与 `api/ipc.rs` 的 `signal`/`wait_notification`：

- `signal(ntfn, badge)`：有等待者则唤醒并交付 badge；否则并入 pending bits（多次合并）。**badge 是绑定到该中断线的 Notification cap 的 badge**（root 通常把它 mint 成中断号）。
- 驱动侧：`poll`（非阻塞）或 `wait`（阻塞）取 badge，读设备直到清空事件源（VirtIO 的 used ring + InterruptACK），再 `Ack`。
- 一个 Notification 可绑定多条线，驱动按 cap badge 区分。
- `TCB_BindNotification`（可选）：把 Notification 绑到 TCB，使信号能唤醒阻塞在别处的线程；v1 驱动用 `wait(ntfn)` 即可，故列为可选。

## 7. 用户态驱动迁移（block-server）

现状：`device.read_blocks(...)` 同步轮询。目标：

```text
驱动请求循环:
  READ(lba, count) ->                # 已有协议
     构造 VirtIO 描述符链，QueueNotify
     Wait(ntfn)                      # 等 used ring 完成通知，badge=IRQ
     检查 used ring 的完成项与状态
     Ack(irq_handler)                # 重新开放设备线
     reply(status)
```

VirtIO MMIO 的队列完成中断（`QueueNotify` 后设备置 `InterruptStatus`）绑定到该 Notification。驱动 `InterruptACK`/读 used ring 清源后再 `Ack`。

**驱动契约（硬性）：`SetNotification` 之前必须先清设备的中断锁存**（VirtIO 即读一次 `InterruptACK`）。内核在 bind 时无条件重新使能 GIC 线——这是有意的加强，让重启的后继驱动能恢复前任 `Clear` 掉的线；代价是：设备若因预绑定阶段的完成（或前任崩溃）保持着中断断言，边沿线没有新沿就永远不会投递。block-server 的绑定路径因此先 `device.ack_interrupt()` 再 bind。

验收要点（§14）：重复/合并通知不丢完成项（一次 Wait 后遍历整个 used ring，不只处理一项）；DMA 在撤销/复用缓冲前必须静止（服务重启路径已有 `drop(device)` + STOP 握手，保持不变）。

## 8. 安全边界

- **`IRQControl` 只给 root**：`init_root` 发布；普通任务无此 cap。
- **`IRQHandler` 可被显式授予/派生**：root 交给 init，init 按服务授予（与设备 Untyped 同样的按服务副本模式）。
- **驱动没有 GIC MMIO**：设备 Untyped 只发布 UART/VirtIO；GIC 永不发布，中断完全经内核仲裁。
- **中断风暴防护（GICv3）**：投递后中断保持 active，直到驱动 `Ack` deactivate；`Clear`/删除 handler 把线 disable。未知源立即 EOIR+DIR 回收。电平源未清不会高速重入内核。
- **保留线**：timer/内核专用线拒绝 `Get` 与重复绑定。

## 9. 分阶段实施

| 阶段 | 内容 | 前置 |
| --- | --- | --- |
| I0 | `Object::IrqControl`/`IrqHandler`、`INIT_IRQ_CONTROL`、平台中断表、`IRQControl_Get`（校验 + 目标槽） | 现有对象模型 |
| I1 | `IRQHandler_SetNotification`/`Clear`/`Ack`；`interrupt.rs` 投递路径（signal → priority drop）；`arch/machine/gic.rs` 从 combined EOI 改为 split（priority_drop + deactivate） | I0、Notification 已有 |
| I2 | root/userboot/init 的 IRQHandler 授权链：root 持有 `IRQControl`，按服务授 `IRQHandler` + Notification | I1 |
| I3 | block-server 从轮询改为通知等待；VirtIO 完成中断绑定 | I2 |
| I4 | 风暴/合并/保留线/权限负向用例；`TCB_BindNotification`（可选） | I3 |

## 10. 测试与验收

- **I0**：非法线、内核保留线、重复 `Get`、错误目标槽全部失败；合法 `Get` 生成可用 `IRQHandler`。
- **I1**：软件置 pending 一条 SPI → 绑定的 Notification 收到 badge（绑定时 mint 的值）；投递后中断保持 active（未 deactivate、未 mask）；`Ack` 后 deactivate，源仍有效时能再次投递。
- **I1 风暴**：电平源未清，`Ack` 后再次触发，但每次都要走一遍内核投递 + 驱动处理，不会在一次处理内无限重入（active 状态阻止重入）。
- **I2**：普通任务无 `IRQControl`；只有被授予的 `IRQHandler` 可用；驱动无 GIC 映射（直接访问故障）。
- **I3**：block-server 在完成中断路径下读盘正确；重复/合并通知不丢 used ring 完成项。
- **回归**：现有 `check_block`/`check_fat32`/`check_appmgr`/`check_restart`/`check_fault_handler` 全绿；新增 `tools/check_irq.py`。
- 覆盖 debug/release × LOG=off/info。

## 11. 与其他设计的关系

| 文档 | 修订 |
| --- | --- |
| [interrupts.md](interrupts.md) | 补充"用户 IRQ 投递"一节，取代"尚未实现"的表述 |
| [disk-driver.md](disk-driver.md) §6/§8 | block-server 从轮询 → 通知；验收 D5 增加 IRQ 路径 |
| [service-manager.md](service-manager.md) | init 按服务授予 `IRQHandler`+Notification；阶段 F 的 IRQ 候选项转正 |
| [microkernel-design.md](microkernel-design.md) §14 | 本阶段完成后勾掉"设备中断" |
| [evolution-plan.md](evolution-plan.md) 阶段 5 | IRQ 授权落地 |

## 12. 开放决策

1. 触发方式来源：平台静态表（建议）vs 从 DTB 读 `interrupts` 属性。
2. `IRQControl_Get` 是否需要 `IRQControl_GetTrigger`（seL4 部分平台有）：v1 用平台表，不暴露给用户。
3. `Ack` 与 `Clear` 对未绑定线/未 mask 线的幂等语义。
4. 一个 `IRQHandler` 是否允许重复 `SetNotification`（覆盖 vs 拒绝）。
5. `TCB_BindNotification` 是否在 v1 实现（驱动用 `Wait` 即可，倾向延后）。

## 13. 实施记录（2026-09-10）

I0–I4 全部落地，`make check` 全绿（含新增 `tools/check_irq.py`）。

### 13.1 落地内容

- **对象与 ABI（I0）**：`Object::IrqControl`/`Object::IrqHandler`（`kernel/src/object/irq.rs`），`INIT_IRQ_CONTROL = 4`，Invocation 26–29。`IrqHandler` 只存 `irq` 与 `bound`——§3 的 `trigger` 字段省略：触发方式在 `Get` 时按平台表编程进 GIC，此后无路径可改，无需存对象。`IrqHandler` 不是 Untyped retype 类型，只收元数据预算（无物理开销）。
- **平台表（I0，后经评审通用化）**：`build_platform.py` 从 DTB 生成**一条一记录**的 `IRQ_LINES`（VirtIO 槽位线升序在前、PL011 线在后，见 §3.1），并经 BootInfo 记录 9（`IrqDesc`，`InitialTaskLayout` 按 `MAX_IRQ_LINES=64` 预留）发布给 root；userboot 按记录逐条 `Get` 建 master，不再探测 0..1020。授权策略是生成的表数据，不是对窗口常量做算术；timer PPI（INTID 30）与其余全部拒绝。§12.1 按“平台静态表”落地。
- **GIC split EOI（I1）**：`gic.rs` 初始化改 `set_eoi_mode(true)`；`complete` 拆为 `priority_drop`（仅 EOIR，凭证消费）与 `priority_and_deactivate`（EOIR+DIR），另有 `deactivate(irq)` 供用户 `Ack` 直接写 DIR。投递路径（`interrupt.rs`）：timer → 重装 + 立即回收 + Reschedule；有绑定 → `signal_notification` + `priority_drop`（唤醒等待者时 Reschedule）；未知源 → 立即回收 **并屏蔽**（§5 已补记；保留原防风暴行为，即 seL4 对 IRQInactive 的 mask）。
- **投递语义（I1）**：`signal` 返回是否唤醒了等待者；badge 取绑定 cap 的 badge（root 用 `CNodeMint` 铸成中断标识）。绑定 Notification 参与 GC 存活边（handler 未清绑定时对象不回收）；handler 对象被收集时 `retire`：移除投递索引 + 屏蔽 + deactivate。
- **授权链（I2，后经评审通用化）**：root 持 `IRQControl`；userboot 按 BootInfo IRQ 记录逐条 `Get`（内核平台表是唯一事实来源，记录即数据，无需探测），在根 CSpace 建 33 个 master（170 起，含 UART 线），整窗授予 init（init 侧 800 起）。init 按服务复制（每服务 `5002+i*8`，随设备 copies 同模式：teardown 时 revoke 副本、对 master `Clear`），经 `SpawnInfo.extra[IRQ_SLOT]=56` 授给 block-server。
- **block-server（I3）**：从自身预算 retype Notification、mint 出带 badge 副本（badge 非 0，否则无等待者的 signal 会并进空 bits 丢失）、`SetNotification` 绑定、`enable_interrupts`。READ 路径：`read_blocks_nb` → `recv(ntfn)` → **遍历整个 used ring**（合并通知不丢完成项）→ `ack_interrupt()`（先清设备源）→ `IrqHandler::ack`（DIR）→ 回复。`BLK_TEST=1` 的验收钩子刻意保留在绑定**前**，继续走轮询路径。
- **验收（I4）**：`tools/check_irq.py` 新增并进 `make check`：GDB 劫持 root 任务，用 `Qqemu.PhyMemMode` 直写 GICD `ISPENDR` 软件置 pending、直读 `ISENABLER/ISACTVR` 观察线状态；覆盖平台表负向、重复 `Get`（`REVOKE_FIRST`）、占用槽（`ALREADY_MAPPED`）、错误 cap 类型、badge 投递、active 阻止重投、Ack 后重投、`Clear` 后静默、重绑定恢复（重启路径）、子任务无 `IRQControl`、内存无泄漏。内核侧 `test::interrupt::authorization_self_test` 由 `check_user_context` 执行（它跑在 `task::init` 里，`check_kernel` 的 `start_root` 断点之前不会执行），驱动与 wire 标签同一套内部接口。

### 13.2 §12 决策结论

1. **触发方式来源**：平台静态表，构建时由 DTB 生成（`build_platform.py` 校验 SPI 连续与触发一致）。
2. **`GetTrigger`**：不暴露；触发方式对用户不可见也不可选。
3. **`Ack`/`Clear` 幂等**：`Ack` 幂等（对非 active 线写 DIR 被 GIC 忽略）；`Clear` = 解绑 + 屏蔽 + **deactivate**（比 seL4 更强：清除崩溃驱动遗留的 active 态，保证重授权的线立即可投递）。
4. **重复 `SetNotification`**：覆盖并重新使能线——重启的驱动重绑即恢复前任 `Clear` 掉的线。
5. **`TCB_BindNotification`**：延后（v1 驱动用 `Wait` 足够）；Invocation 13/14 未占用。

### 13.3 实施中确立的平台事实

- **QEMU virt 把 virtio 设备挂到窗口的最后一个槽位**：`-device virtio-blk-device` 落在 `0x0a003e00`（slot 31，SPI 47/INTID 79），而不是 slot 0。设备名到中断线的映射因此是 `virtio-mmio-N → 线 count-1-N`（从窗口末端起分配），由 `probe` 逐槽记录设备类型实证。加第二块盘预期占用 slot 30。
- **设备线是边沿触发**（DTB flags=1），而 QEMU 的 virtio-mmio 模型保持电平直到驱动读 `InterruptACK`。两者叠加出一条硬规则：**绑定（或重绑）前必须先 `ack_interrupt` 清设备锁存**——预绑定阶段的完成中断（或前任崩溃遗留的 ISR）会把线拉高，边沿线没有新沿就永远不会投递。该规则已写进 block-server 绑定路径。
- **Notification 合并是按位 OR，同一 badge 的重复投递不可区分**（`0x40 | 0x40 == 0x40`）。验证“Ack 后重投”必须先清空 pending bits（驱动 `wait` 天然如此，内核测试与 `check_irq` 均按此写）； DIR 写本身的行为已由状态直读证实：active 清除、latched pending 保留。
- 预绑定阶段的完成中断会命中内核“未知源”路径（回收 + 屏蔽），驱动的 `SetNotification` 重新使能即恢复——这也是 §13.1 决策 4 覆盖语义的由来。

### 13.4 与设计的偏差汇总

- `IrqHandler` 不存 `trigger`（§3）；`Clear` 额外 deactivate（§12.3）；未知源在回收后仍屏蔽（§5 原稿未写明，现已补进伪代码）；`bound` 存 `(ObjectId, badge)` 二元组（§3 原稿只写 `ObjectId`，badge 是投递所需）。占用槽错误码不是偏差：`ALREADY_MAPPED` 的 wire 值 8 与 seL4 `DeleteFirst` 相同。
- 用户态串口（evolution-plan 阶段 5 的另一半）不在本文范围。PL011 的中断线（INTID 33，电平）已列入平台表并可授权，console 服务暂维持轮询——后续切 RX 中断只需给 console 服务授线/绑定，无需再动平台层。

### 13.5 评审后通用化（2026-09-11）

评审指出首版把平台表特化成了"VirtIO 常量算术 + 0..1020 探测"，两处都已按数据化重构：

- **平台表 → 生成的一条一记录**：`IRQ_LINES: &[(intid, level, kind)]`（见 §3.1），`platform_trigger` 变成查表；新增 PL011 线。加第二种设备只需生成器多认一个节点，内核与 supervisor 零改动。
- **探测 → BootInfo 记录**：`BOOTINFO_HEADER_IRQS=9`/`IrqDesc` 由内核写入、runtime 解析（`BootInfo::irq_lines`），userboot 按记录建 master 并把 kind-0 计数传给 init（`extra[IRQ_SLOT+1]`，语义不变）。去掉 O(1020) 探测与"连续性 → master i = slot i"的隐式假设——映射现在依赖记录顺序（数据契约），不再依赖线号连续。
- `server::wait_irq()` 未实现：`ipc::recv` 即通知等待，不加转发层。
- 占用槽错误码说明修正：`ALREADY_MAPPED` 的 wire 值 8 与 seL4 `seL4_DeleteFirst` 相同，非偏差（§13.4）。
- "bind 前清设备锁存"升格为 §7 的硬性驱动契约，并写入 `IrqHandler::set_notification` 的 rustdoc。
