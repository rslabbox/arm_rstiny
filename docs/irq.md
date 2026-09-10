# 设备 IRQ 授权与用户态投递

日期：2026-09-14。状态：设计提案，待实施。

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

中断的触发方式（level/edge）与是否可授权由**平台表**决定，不允许用户指定：

```rust
// 由 config/平台生成提供
const IRQ_TABLE: &[(u32, Trigger, bool /* user-visible */)] = &[
    // (intid, trigger, user_visible)
    (TIMER_IRQ, Trigger::Level, false),   // 内核保留
    // 其余 SPI 来自 DTB / 平台配置，默认用户可见
];
```

`IRQControl_Get` 只接受表中 `user_visible = true` 且未被占用的线。

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

`INIT_IRQ_CONTROL = 4` 加入初始 cap 布局。错误码沿用现有集合：非法线/内核保留 → `InvalidArgument`；重复 `Get` 一条已 active 的线 → `RevokeFirst`（seL4 用这个名字）；目标槽非空 → `DeleteFirst`；错误对象类型 → `InvalidCapability`；无 `Write` 权 → `PermissionDenied`。`Get` 成功即 `setIRQState(IRQSignal)`，把线 **enable**（允许投递）。

用户态封装放在 `projects/libs/user/src/capability.rs`：`IrqControl::get`、`IrqHandler::{set_notification, ack, clear}`；`projects/libs/server` 提供 `wait_irq()` 便捷函数。

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
