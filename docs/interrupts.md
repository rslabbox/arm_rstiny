# 中断控制器、定时器与调度 tick

当前实现固定为 AArch64 QEMU virt、GICv3、单 CPU。内核执行期间 IRQ 始终屏蔽；中断可从 EL0 返回，也可在 masked WFI 唤醒后处理。

## 模块职责

| 模块 | 职责 |
| --- | --- |
| `arch/machine/gic.rs` | GIC 初始化、使能、触发方式、优先级、pending、领取与完成 |
| `arch/machine/instructions.rs` | CPU 的 PSTATE.I 查询和 masked WFI |
| `arch/machine/time.rs` | 系统计数器读数与频率，不依赖中断控制器 |
| `arch/machine/timer.rs` | CNTP 单次定时器：初始化、绝对到期时间、停止 |
| `interrupt.rs` | 根据中断源分发处理，并在完成中断后返回调度决定 |
| `task/tick.rs` | 调度周期、timer PPI 配置和周期重装策略 |
| `task/runtime.rs` | 根据分发结果继续当前用户上下文或进入调度点 |

分层参考 seL4 的 GIC `getActiveIRQ/maskInterrupt/ackInterrupt`、独立 generic timer、通用 `handleInterrupt`，以及 x-kernel 的 GIC 接口、timer driver 与 kirq 分发。没有引入 IRQ domain、NMI、SMP 或动态 handler 注册。

## GIC 接口与生命周期

GIC 模块独占 `arm-gic-driver` 的 `Gic` 和 `CpuInterface`。公开的内核内部接口是 `init`、`set_enable`、`set_trigger`、`set_priority`、`set_pending`、`claim`、`priority_drop`、`priority_and_deactivate`、`deactivate`、`from_intid`；它不导入 timer 或 scheduler。

`IrqId` 与 `Trigger` 复用底层库的类型。私有中断操作走当前 CPU interface，共享中断走 distributor。固定平台只使用普通 SGI/PPI/SPI；不支持 LPI 或扩展中断号。

`claim()` 读取 IAR，特殊/伪中断返回 None，不执行 EOI。有效中断返回不可复制、不可由外部构造的 `ActiveInterrupt`。控制器记录当前 active ID，拒绝尚未完成时再次领取；消费凭证的路径有两条：`priority_drop` 只写 EOIR（投递后保持 active，用户 IRQ 用），`priority_and_deactivate` 写 EOIR + DIR（timer 与未知源用，立即回收）。用户 `Ack` 经 `deactivate` 单独写 DIR，幂等（对非 active 线写 DIR 被 GIC 忽略）。

使用 split EOI 模式（`EOImode=1`）：投递只降优先级，中断保持 active——GIC 不会重投同一线，直到驱动 Ack deactivate。这就是用户 IRQ 的隐式屏蔽（[设备 IRQ 授权与用户态投递](irq.md) §5）。

每个控制器操作只短暂借用 `SingleCore`。`claim` 返回前借用已经结束；设备 handler 可安全调用其他控制器操作。任何 active 凭证都必须在 park、eret、WFI 之前完成。没有在 Drop 中隐式 EOI，避免将中断完成隐藏在析构顺序里。

## 定时器与 tick

`timer::init()` 检查计数器频率并停止定时器；`arm(deadline)` 把系统 counter ticks 写入 CNTP_CVAL_EL0，启用且不屏蔽源；`stop()` 禁用并屏蔽定时器。驱动没有周期状态、GIC 依赖或调度逻辑。

绝对 CVAL 支持超过 TVAL 有符号 32 位范围的间隔。已经过去的到期时间立即触发源，时间单位与 `time::now()` 相同。

`task/tick.rs` 将 `config::TICK_NS` 向上取整为计数器 ticks。初始化顺序为：停止 timer → 禁用 PPI → 配置 Level/优先级 → 清除旧 pending → 设置到期时间 → 使能 PPI。当前保留 10 ms 周期策略，每次到期从当前计数重新设置下一次到期；不是 tickless 或 MCS deadline 调度。

## 分发顺序

`interrupt::handle()` 执行：

1. 领取中断；伪中断返回 Continue。
2. timer IRQ 调用 tick handler 重装定时器，随后立即回收（EOIR+DIR）。
3. 有绑定的用户线：signal 绑定的 Notification（badge 取绑定 cap），只降优先级不 deactivate——active 状态阻止重投，直到驱动 Ack。唤醒了等待者时返回 Reschedule。
4. 未知源：立即回收并禁用该线（seL4 IRQInactive 语义，防电平源风暴）。

用户循环只在 Reschedule 时进入调度点；未知中断不会被当成 tick。idle 执行 `instructions::wait_for_interrupt()` 后调用同一分发入口，再回到调度循环检查就绪任务。已经 pending 的可投递中断能够唤醒 masked WFI，不需要在 EL1 临时打开 IRQ。

## 验证

`kernel-test` 在 GIC 初始化后、tick 启动前运行实际硬件测试：软件置 pending 的 SGI 和 SPI、禁用源不可领取、重复领取/完成周期、未知源被屏蔽且完成、伪中断、长单次定时器和已过期源停止；外加 IRQ 授权与投递全套（平台表负向、重复 Get、badge 投递、active 阻止重投、Ack 重投、Clear 静默，见 `test/interrupt.rs::authorization_self_test`）。

`tools/check_user_context.py` 使用测试构建并检查 `IRQ_SELF_TEST_PASSED`，随后执行每配置 2048 次上下文返回，以及 pending-before-WFI 唤醒测试。任务测试验证纯 EL0 循环的定时器抢占、sleep/wait 和暂停恢复。覆盖 debug/release × LOG=off/info。

设备中断的用户态投递（`IRQControl`/`IRQHandler`/Notification 授权）已实现，见 [设备 IRQ 授权与用户态投递](irq.md)（设计与实施记录）。用户态层面的投递/Ack/Clear 语义由 `tools/check_irq.py` 直写 GICD 寄存器验证。
