# 架构目录

`kernel/src/api` 管理用户与内核交互的规则；`kernel/src/arch` 实现 AArch64，按职责分成 `kernel` 和 `machine`。模块引用直接使用职责路径，根模块不转导旧的平铺模块。

```text
api/
  mod.rs
  dispatch.rs                  # 系统调用与对象调用分发
  message.rs                   # 请求校验、IPC buffer、消息和回复编解码
  debug.rs                     # 用户 debug 调用的可用性与输出策略
  faults.rs                    # 故障记录、用户故障处理策略
arch/
  mod.rs
  kernel/
    mod.rs
    boot.rs                    # 原始入口、BSS、loader 交接校验
    trap.rs                    # 异常向量绑定、ESR/FAR 读取和内核致命异常
    trap.S                     # 异常保存与返回
    thread/
      mod.rs
      context.rs               # TrapFrame 布局
      kernel_context.rs        # 内核 continuation 保存与切换
      user.rs                  # UserContext、EL0 进入与事件返回
    vspace/
      mod.rs
      page_table.rs            # AArch64 页表项编码与解码
      paging.rs                # 页表池、遍历、映射及回滚
  machine/
    mod.rs
    instructions.rs            # CPU 屏障相关操作、IRQ mask 查询、WFI、TLBI
    mmu.rs                     # TTBR/TCR/SCTLR 操作、当前硬件地址查询
    gic.rs                     # GICv3 控制器操作
    time.rs                    # 系统计数器与频率
    timer.rs                   # CNTP 单次定时器
```

## 与本地 seL4 的对应关系

参考源码位于 `../seL4/kernel`：

| seL4 路径 | 本项目职责位置 |
| --- | --- |
| `src/api/syscall.c`、`include/api/` | 顶级 `api/dispatch.rs`、`message.rs`、`debug.rs` |
| `src/api/faults.c`、架构 API 头文件 | 顶级 `api/faults.rs` 管理处理策略；`UserContext` 管理本平台调用寄存器约定 |
| `src/arch/arm/kernel/boot.c` | `arch/kernel/boot.rs` |
| `src/arch/arm/64/traps.S`、`c_traps.c` | `arch/kernel/trap.S`、`trap.rs` |
| `src/arch/arm/64/kernel/thread.c`、`include/arch/arm/arch/machine/registerset.h` | `arch/kernel/thread/`；相关寄存器布局与保存恢复代码放在一起 |
| `src/arch/arm/64/kernel/vspace.c` | `arch/kernel/vspace/`，上层地址空间策略继续由 `memory/` 管理 |
| `src/arch/arm/machine/gic_v3.c`、machine/tlb 接口 | `arch/machine/gic.rs`、`instructions.rs`、`mmu.rs` |
| `include/drivers/timer/arm_generic.h`、`src/drivers/timer/generic_timer.c` | `arch/machine/time.rs`、`timer.rs` |

采用 seL4 的职责划分，没有复制其 ARM32/ARM64、不同 ARM 版本、生成头文件和平台选择层。seL4 的通用定时器位于 drivers；当前只有固定 ARM 平台，定时器寄存器操作集中在 machine，调度策略仍在 `task/tick.rs`。

## 边界

- 顶级 `api` 解码共享 ABI、校验消息、调用能力对象、编码回复并决定用户故障处置。错误码与 wire 类型继续使用 `projects/libs/abi`，不复制定义。当前没有 fault endpoint，故障处理仍记录并终止任务，不表示实现了 seL4 故障投递。
- `UserContext` 集中封装 AArch64 调用寄存器：x7 的调用号、x0 的 capability/badge、x1 的 MessageInfo 原始字以及 x2…x5 的消息寄存器。`MessageInfo` 的字段解释、IPC buffer 读取和零 badge 回复规则属于顶级 `api`。这些方法仍通过 `UserContext` 调用，避免在通用分发里直接索引寄存器数组。

- `arch/kernel` 实现内核与 AArch64 的连接机制：启动交接、异常帧、执行上下文和页表结构。它不持有就绪队列，不分配 capability，也不决定用户地址空间的资源授权。
- `arch/machine` 提供硬件操作，不调用调度器或 syscall 分发。GIC 和定时器是两个独立模块，通过上层中断处理和 tick 策略组合。
- `memory/kernel` 继续持有正式内核页表并描述映射布局；`memory/address.rs` 负责基于实际加载 PA 的地址换算；`memory/space.rs` 管理用户页与地址空间所有权。
- `task` 继续管理任务生命周期和运行循环；`api/dispatch.rs` 分发系统调用，`object` 实现能力对象接口。`task/api.rs` 是任务子系统的受控内部操作入口，继续负责当前任务身份、状态和资源访问，和顶级用户 API 职责不同。不会为了模仿 seL4 的目录名称，再建立一套空的 `arch/object`。

`TrapFrame` 从 `arch::kernel::thread` 导出，`PageTableEntry` 从 `arch::kernel::vspace` 导出；异常桥接使用的 `RawTrap` 和 `KernelReturnFrame` 仅在 `arch::kernel` 内可见。

本次调整目录、Rust 模块路径及内部可见性，保留汇编符号、结构布局、系统调用 ABI 和实际启动行为。
