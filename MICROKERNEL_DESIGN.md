# arm_rstiny 纯微内核设计草案

本文档给出面向当前 arm_rstiny 仓库的一版可落地微内核设计。目标不是一次性重写整个系统，而是先固定内核边界、对象模型、syscall 语义和服务拆分方式，再按阶段演进。

## 1. 设计目标

### 1.1 目标

1. 将内核收缩为最小特权机制层。
2. 通过同步 IPC 和 capability 建立清晰的安全边界。
3. 将驱动、文件系统和设备管理逐步迁移到用户态服务。
4. 保持当前 AArch64 启动链、SMP 和基础调度路径可渐进迁移。
5. 为后续容错、服务重启、驱动隔离打基础。

### 1.2 非目标

1. 第一阶段不追求 POSIX 兼容。
2. 第一阶段不实现完整 pager 或 demand paging。
3. 第一阶段不将所有驱动一次性迁出内核。
4. 第一阶段不引入复杂的多级调度策略。

## 2. 当前代码基线

从当前仓库看，已经具备微内核化所需的几块基础能力：

1. 启动和异常入口已经集中在 boot 和 hal 中。
2. TrapFrame 已经提供 AArch64 syscall 所需寄存器访问接口。
3. 调度框架、per-cpu 和线程上下文已经存在。
4. provider 机制已经把内核内部设备能力做了一层抽象。

这意味着当前最缺的不是底层汇编入口，而是：

1. 内核对象模型。
2. syscall 分发层。
3. IPC 机制。
4. capability 空间。
5. 用户态服务启动链。

## 3. 微内核边界

### 3.1 保留在内核中的内容

纯微内核内核态只保留以下机制：

1. CPU 和异常入口管理。
2. 线程调度与上下文切换。
3. 地址空间和页表管理。
4. IPC 对象和消息投递。
5. capability 验证与对象引用。
6. 中断接收、最小确认和事件路由。
7. 最小时间源和 CPU 本地定时器接入。
8. 根任务启动和用户态初始映像装载。

### 3.2 从内核移出的内容

下面这些都应作为用户态服务长期存在：

1. UART 服务。
2. 块设备服务。
3. 文件系统服务。
4. 设备管理服务。
5. FDT 解析后的策略性绑定逻辑。
6. Shell 与命令系统。

## 4. 内核对象模型

建议内核第一阶段只支持以下对象类型。

### 4.1 Task

保护域对象，拥有：

1. 一个地址空间。
2. 一个 capability 空间。
3. 一组线程。
4. 一组映射关系。

Task 是资源和权限的承载者，不直接等同于可调度实体。

### 4.2 Thread

线程对象是实际被调度的执行实体，拥有：

1. TrapFrame。
2. TaskContext。
3. 所属 Task。
4. 调度状态。
5. 阻塞原因。

线程状态建议至少包括：

1. Ready
2. Running
3. BlockedSend
4. BlockedRecv
5. BlockedReply
6. BlockedNotify
7. Sleeping
8. Exited

### 4.3 Endpoint

同步 IPC 端点，用于 request/reply 通信。

核心语义：

1. 客户端对 endpoint 执行 call。
2. 服务端在线程上执行 recv。
3. 服务端处理后 reply。
4. 内核负责消息复制、线程配对与状态切换。

### 4.4 Notification

轻量事件对象，用于：

1. 中断转发。
2. 定时器事件。
3. 异步唤醒。

Notification 不承载复杂负载，优先使用位图或单词事件掩码。

### 4.5 VmSpace

地址空间对象，表示用户态页表及其映射关系。

需要支持：

1. 建立映射。
2. 解除映射。
3. 切换页表。
4. 查询权限。

### 4.6 Frame

页帧对象，表示一段受管理的物理页。

第一阶段不需要做复杂分段，只要求：

1. 页对齐。
2. 可映射到某个 VmSpace。
3. 拥有只读、可写、可执行、设备页等属性。

### 4.7 Capability

Capability 是用户态访问内核对象的唯一入口。

Capability 记录：

1. 对象类型。
2. 对象引用。
3. 权限位。
4. 是否可派生。

建议的基础权限位：

1. Read
2. Write
3. Grant
4. Map
5. Send
6. Recv
7. Signal
8. Control

## 5. syscall ABI

### 5.1 AArch64 调用约定

当前 TrapFrame 已经符合一版简单 syscall ABI：

1. x8 传 syscall 编号。
2. x0 到 x5 传前六个参数。
3. x0 返回结果值。
4. 使用 SVC 指令进入 EL1。

这意味着当前架构不需要额外改 trap frame 格式，就可以开始接 syscall 分发。

### 5.2 第一阶段最小 syscall 集

第一阶段 syscall 不提供高层服务，只提供机制。

建议编号如下：

```text
0  yield
1  thread_exit
2  endpoint_call
3  endpoint_recv
4  endpoint_reply
5  notify_wait
6  notify_signal
7  map
8  unmap
9  cap_copy
10 cap_drop
11 irq_bind
12 irq_ack
```

说明：

1. yield 用于主动让出 CPU。
2. thread_exit 用于线程退出。
3. endpoint_call 是最常用请求路径。
4. endpoint_recv 由服务线程等待消息。
5. endpoint_reply 用于对 call 回复。
6. notify_wait 和 notify_signal 用于事件同步。
7. map 和 unmap 是最小地址空间管理入口。
8. cap_copy 和 cap_drop 用于 capability 生命周期管理。
9. irq_bind 和 irq_ack 用于中断向 notification 转发。

### 5.3 第二阶段补充 syscall

当第一阶段稳定后，再增加：

```text
13 task_create
14 thread_create
15 thread_start
16 endpoint_create
17 notification_create
18 frame_alloc
19 frame_retype
20 cap_grant
21 cap_revoke
```

第一阶段可以先由根任务拥有预创建对象，降低实现复杂度。

## 6. IPC 设计

### 6.1 采用同步 IPC

第一阶段建议只做同步 IPC，而不做复杂异步邮箱。

原因：

1. 更容易实现正确的线程状态机。
2. 便于建立 request/reply 服务模型。
3. 更适合文件系统和驱动服务。

### 6.2 消息格式

建议最小消息格式如下：

```text
MessageHeader {
    label: u32,
    flags: u16,
    words: u16,
}

Message {
    header,
    mr[0..5],
}
```

第一阶段先限制为固定寄存器消息：

1. 最多 6 个 machine words。
2. 不做变长内核缓冲区。
3. 大块数据通过共享页传递。

### 6.3 Endpoint 语义

服务线程对 endpoint 执行 recv 时：

1. 若没有 pending sender，则线程进入 BlockedRecv。
2. 若有 sender，内核复制消息到接收线程上下文。
3. call 方进入 BlockedReply。
4. 服务线程运行处理逻辑。

服务线程 reply 时：

1. 内核把返回值写回客户端。
2. 唤醒客户端。
3. 服务线程返回 Ready 或继续 recv。

### 6.4 Notification 语义

Notification 采用累积位图语义：

1. signal 时置位。
2. wait 时若非空则直接返回并清位。
3. 若为空则线程进入 BlockedNotify。

这样适合中断事件和定时器事件聚合。

## 7. 内存与地址空间

### 7.1 第一阶段内核负责页帧分配

先不做完整 pager，采用过渡模型：

1. 内核负责页帧分配。
2. 内核负责页表建立。
3. 用户态通过 capability 请求 map/unmap。
4. 缺页异常先视为致命错误或交由根任务兜底。

### 7.2 映射模型

建议最小映射接口支持：

1. Frame -> VmSpace at Vaddr。
2. 映射权限位：R/W/X/Device/User。
3. 可选共享映射。

这样已经足够支持：

1. 用户态文本与数据装载。
2. IPC 共享页。
3. MMIO 受控映射到用户态驱动。

### 7.3 用户态驱动的 MMIO 映射

微内核下用户态驱动不能直接拿物理地址裸映射，必须通过受控能力下发：

1. 根任务或设备管理服务获得设备资源描述。
2. 内核根据策略创建 device frame capability。
3. 驱动服务持 capability 调 map。
4. 内核把对应设备页映射到驱动地址空间。

## 8. 中断路径

### 8.1 内核中的中断职责

内核对中断只做最小工作：

1. 从 GIC 读取中断号。
2. 找到绑定的 notification。
3. signal 对应 notification。
4. 必要时执行最小 ack 或 EOIR。
5. 触发调度点。

### 8.2 用户态驱动中的中断职责

设备具体处理逻辑应由用户态驱动完成：

1. 等待 notification。
2. 读取设备寄存器。
3. 处理队列或状态。
4. 回复上层服务请求。

这样可以把复杂寄存器状态机从内核剥离出去。

## 9. 启动流程重排

当前代码的启动流程仍然偏单体内核。微内核版建议重排为下面几个阶段。

### 9.1 Stage 0: 早期引导

保留在 boot 中：

1. 清 BSS。
2. 建立异常向量。
3. 初始化早期串口。
4. 建立初始页表。
5. 每 CPU 基础状态初始化。

### 9.2 Stage 1: 核心内核对象初始化

内核启动后第一批初始化内容应该是：

1. scheduler。
2. capability allocator。
3. object table。
4. vmspace allocator。
5. endpoint 和 notification 基础池。

### 9.3 Stage 2: 平台基础设施

在保持最小范围内初始化：

1. GIC。
2. timer。
3. SMP bringup。
4. FDT 原始数据保留。

### 9.4 Stage 3: 根任务启动

内核创建初始用户态根任务 rootsrv，并为其注入：

1. 初始地址空间。
2. 根 capability 集。
3. 启动参数。
4. FDT 只读映射或解析结果。

### 9.5 Stage 4: 用户态服务启动

根任务启动以下服务：

1. devmgr
2. uartsrv
3. blksrv
4. fssrv
5. shell

只有在这些服务启动后，系统才具备完整用户交互和存储能力。

## 10. 服务划分

### 10.1 Root 服务

rootsrv 负责：

1. 创建基础服务。
2. 分发 capability。
3. 维护命名服务。
4. 管理系统级策略。

### 10.2 Device Manager

devmgr 负责：

1. 解析 FDT。
2. 枚举设备。
3. 决定某设备交给哪个驱动服务。
4. 为驱动申请 IRQ 和 MMIO 资源。

### 10.3 UART 服务

uartsrv 负责：

1. 串口寄存器访问。
2. 中断收发。
3. 为 shell 和日志服务提供 RPC 接口。

建议把它作为第一个迁出内核的服务。

### 10.4 Block 服务

blksrv 负责：

1. VirtIO 块设备协议处理。
2. 请求队列管理。
3. 对文件系统服务提供块读写 RPC。

### 10.5 File System 服务

fssrv 负责：

1. 文件系统挂载。
2. 目录与 inode 管理。
3. 对应用提供 open/read/write 类 RPC。

## 11. 调度模型

第一阶段调度器保持简单可预测。

### 11.1 调度策略

建议采用：

1. 每 CPU runqueue。
2. 固定优先级。
3. 同优先级 FIFO。
4. 时钟中断抢占。
5. IPC 和 notify 触发唤醒。

### 11.2 调度点

建议在以下时机进入调度判断：

1. timer tick。
2. syscall 阻塞。
3. reply 唤醒高优先级线程。
4. 中断返回前。

## 12. 针对当前仓库的迁移顺序

### Phase 1: 建立机制，不搬服务

目标：

1. 新增 syscall 分发模块。
2. 新增 endpoint 和 notification 对象。
3. 新增 capability 表。
4. 新增最小 vmspace 抽象。

这一阶段结束时，系统仍可保留现有内核驱动。

### Phase 2: 建立根任务

目标：

1. 支持从内核跳到首个用户态任务。
2. 建立用户栈和用户 TrapFrame。
3. 能从用户态发起 yield 和简单 IPC。

### Phase 3: UART 服务化

目标：

1. 把 UART 从 provider 后端迁到用户态。
2. 中断通过 notification 交给 uartsrv。
3. shell 通过 IPC 使用 uartsrv。

这是整个微内核化最重要的第一个验证点。

### Phase 4: 块设备服务化

目标：

1. 把 VirtIO block 迁到 blksrv。
2. 内核只保留 IRQ 路由和 MMIO 映射授权。
3. 文件系统改为通过 IPC 访问块服务。

### Phase 5: 文件系统服务化

目标：

1. 把当前 fs 模块迁成 fssrv。
2. 用户命令通过 RPC 访问文件系统。
3. 内核完全退出高层存储逻辑。

## 13. 建议的源码目录演进

建议新增以下目录：

```text
src/
  kernel/
    object/
      cap.rs
      endpoint.rs
      notification.rs
      task.rs
      thread.rs
      vmspace.rs
    syscall/
      abi.rs
      dispatch.rs
      mod.rs
    ipc/
      message.rs
      mod.rs
    sched/
      mod.rs
  userland/
    root/
    uart/
    blk/
    fs/
```

为了降低第一阶段改动量，可以先把这些模块挂在现有 src/task、src/mm、src/hal 旁边，等接口稳定后再重排目录。

## 14. 第一阶段实现优先级

如果只做最关键的第一批改动，优先顺序应为：

1. syscall 分发入口。
2. 内核对象标识和 capability 表。
3. endpoint 和 notification。
4. 用户态线程进入路径。
5. 简单用户态测试程序。

只有这五件事打通，后面的服务化才是可执行的工程，而不是架构设想。

## 15. 结论

对 arm_rstiny 来说，纯微内核的关键不是先把所有驱动搬走，而是先把最小机制层站稳：

1. syscall
2. IPC
3. capability
4. vmspace
5. root task

这五部分完成后，系统才真正从“带模块化驱动的单体内核”跨入“具备服务化能力的微内核”。