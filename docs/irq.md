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
| `IRQHandler_Ack` | `irq_handler` | 重新开放该线（此前被内核 mask） |
| `IRQHandler_Clear` | `irq_handler` | 解除绑定 |
| 投递 | 内核收到中断 → 找到 handler → mask 该线 → `Signal(ntfn, badge=irq)` | 驱动读设备清源后 `Ack` |

关键规则：

1. **中断线在投递后保持 mask，直到驱动 `Ack`。** 这防止电平中断在源未清除时无限重入。
2. **`badge` 携带中断号**，一个 Notification 可服务多条线，驱动按 badge 区分。
3. **`IRQControl` 只给 root**；普通任务只能拿到被显式授予的 `IRQHandler`/Notification cap。
4. **定时器与内核专用中断不可被用户 `Get`**，重复绑定被拒绝。
5. 驱动的 Notification 与 GIC MMIO 分离：驱动完全没有 GIC 映射，只能通过 `Ack` 请求内核重新开放线路。

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

`INIT_IRQ_CONTROL = 4` 加入初始 cap 布局。错误码沿用现有集合：非法线/内核保留 → `InvalidArgument`；已被占用 → `DeleteFirst`（或 `RevokeFirst`，实现时定）；错误对象类型 → `InvalidCapability`；无 `Write` 权 → `PermissionDenied`。

用户态封装放在 `projects/libs/user/src/capability.rs`：`IrqControl::get`、`IrqHandler::{set_notification, ack, clear}`；`projects/libs/server` 提供 `wait_irq()` 便捷函数。

## 5. GIC 时序

GICv3 采用 combined EOI（`complete()` 同时降优先级 + deactivate）。投递一条用户中断：

```text
claim()                       -> ActiveInterrupt(id)     # 读 IAR
if id == timer: tick.rearm(); complete(id); reschedule
else if handler(id) bound:
    gic.set_enable(id, false)  # mask，直到 Ack
    notification.signal(badge = id)
    gic.set_pending(id, false) # 电平：清除再等源；边沿：无操作
    complete(id)               # EOI（降优先级 + deactivate）
else:
    gic.set_enable(id, false)  # 未知源：屏蔽，防风暴
    complete(id)
```

`IRQAckIRQ(handler)`：

```text
if !handler.bound: Err(InvalidState)
gic.set_enable(handler.irq, true)    # 重新开放；源未清则再次触发
```

规则与边界：

- **mask 在 `complete` 之前**，避免 EOI 后立刻重入。
- 电平中断：驱动必须先清设备源再 `Ack`，否则立即再次触发（测试要覆盖"不清源会重触发但受 mask 限制、不会高速风暴"）。
- `IRQHandler_Clear`：解绑，并把线保持 mask（不自动重新开放，避免无主中断风暴）。
- SPURIOUS/特殊 ID 不 EOI，不投递。
- 定时器 PPI 与内核专用线永不可 `Get`。

## 6. Notification 交付

复用现有 `Object::Notification` 与 `api/ipc.rs` 的 `signal`/`wait_notification`：

- `signal(ntfn, badge)`：有等待者则唤醒并交付 badge；否则并入 pending bits（多次合并）。
- 驱动侧：`poll`（非阻塞）或 `wait`（阻塞）取 badge，读设备直到清空事件源（VirtIO 的 used ring），再 `Ack`。
- 一个 Notification 可绑定多条线，badge = INTID 区分。
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
- **中断风暴防护**：投递后 mask，直到 `Ack`；未知源直接 mask；电平源未清不会高速重入内核。
- **保留线**：timer/内核专用线拒绝 `Get` 与重复绑定。

## 9. 分阶段实施

| 阶段 | 内容 | 前置 |
| --- | --- | --- |
| I0 | `Object::IrqControl`/`IrqHandler`、`INIT_IRQ_CONTROL`、平台中断表、`IRQControl_Get`（校验 + 目标槽） | 现有对象模型 |
| I1 | `IRQHandler_SetNotification`/`Clear`/`Ack`；`interrupt.rs` 投递路径（mask → signal → EOI） | I0、Notification 已有 |
| I2 | root/userboot/init 的 IRQHandler 授权链：root 持有 `IRQControl`，按服务授 `IRQHandler` + Notification | I1 |
| I3 | block-server 从轮询改为通知等待；VirtIO 完成中断绑定 | I2 |
| I4 | 风暴/合并/保留线/权限负向用例；`TCB_BindNotification`（可选） | I3 |

## 10. 测试与验收

- **I0**：非法线、内核保留线、重复 `Get`、错误目标槽全部失败；合法 `Get` 生成可用 `IRQHandler`。
- **I1**：软件置 pending 一条 SPI → 绑定的 Notification 收到 badge = INTID；线被 mask；`Ack` 后重新可投递。
- **I1 风暴**：电平源未清，`Ack` 后再次触发但受 mask 限制，不会在一次驱动处理内无限重入。
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
