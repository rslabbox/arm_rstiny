# userboot 与 init 服务管理设计

日期：2026-09-09。状态：阶段 A–C 已实施并通过回归（内核 IPC/fault/Untyped 切分、boot module archive、userboot→init→console 链），阶段 D–F 未实施。本文在原提纲基础上补齐了实现级设计：内核 Endpoint/Notification/fault endpoint、Untyped 切分、BootInfo v6 与 boot module archive、初始 capability 布局、控制协议 wire 格式、init/libs/server 内部结构和分阶段文件改动清单。实现事实以源码为准，文末附实施记录。

本文把启动链从"kernel → fatboot（root task 兼驱动/FS）"改为"kernel → userboot → init（service manager）→ 服务/应用"。设计目标、组件职责、能力模型、内核机制、协议、配置格式、重启语义和分阶段验收在此固定；实现事实以源码为准。相关背景见 [对象内存所有权模型](object-ownership.md)、[Untyped 物理内存实现计划](untyped-plan.md)、[seL4 风格 ABI](sel4-abi.md)、[完整微内核设计与分阶段路线](microkernel-design.md)、[架构目录](arch-layout.md)。

## 1. 目标与范围

目标：

- `fatboot` 改名 `userboot`，职责收缩为 bootstrap + 监督 init，不含任何驱动或文件系统。
- 新增 `init`（service manager）：按文本配置起服务、收集 report、按策略重启崩溃的服务。
- 新增独立的 `appmgr`：在 `fs-server` 就绪后从文件系统加载应用。
- 服务（console/block/fs）与应用各自独立地址空间，通过 IPC 通信。
- 沿用现有对象/capability/Untyped/设备 Untyped 模型，内核机制按 seL4 non-MCS（AArch64、单核、无 MCS/SMP/SMMU）语义对齐，不引入 seL4 二进制兼容。

范围外：MCS/优先级、SMP、网络、多级 CNode、完整 MDB、POSIX、动态链接、cap unwrap、可转移 reply cap、绑定 Notification。这些在对应小节标注为差异或延后项。

## 2. 启动链与两级监督

```text
bootloader
  └─ kernel (EL1)
       └─ userboot        (root task, EL0)      bootstrap + 监督 init
            └─ init       (service manager)      起服务 + 收 report + 重启
                 ├─ console_server
                 ├─ block-server
                 ├─ fs-server
                 └─ appmgr                        从 fs 加载应用
```

两级监督：

| 级别 | 监督者 | 被监督者 | 动作 |
| --- | --- | --- | --- |
| 1 | userboot | init | init 崩溃/退出时重建并重启，有限次 + 退避 |
| 2 | init | services / appmgr | 按服务策略重启，超过阈值标记 Failed |

userboot 保留为 monitor（决策 1）：它不参与服务管理，但在 init 失效时提供最后一道防线。

## 3. 组件职责

### 3.1 userboot（root task）

做：

- 接收 BootInfo（untyped、设备 untyped、IPC buffer、FDT、boot module archive）。
- 把 archive 页只读映射进自己的地址空间，解析 archive，定位 `init.elf` 与 `init.cfg`。
- 用用户态 ELF loader 创建 init（独立 VSpace/CSpace/TCB）。
- 向 init 授予：
  - 一个普通 untyped 中切出的子 untyped（init 预算）；
  - 全部设备 untyped；
  - archive 的只读 Frame cap（init 的 ROM）；
  - init 自身的 TCB/CSpace/VSpace cap；
  - userboot `control_ep` 的 badged cap（init 用作 fault + report + supervision 回执）；
  - SpawnInfo 参数页（见 6.2）。
- 保留：
  - `bootstrap reserve` untyped 与 init 预算子 untyped 的母 cap 副本（Copy 给 init 而非 Move）：init 失效时 userboot `Revoke` 母副本即可一次回收 init 全部对象并重置该子区间 watermark，再重建 init；
  - init 的 fault endpoint（即 control_ep 对象，由 userboot 创建并持有母 cap）；
  - 自己的栈与监督循环。
- 进入 monitor：只等 init 的 fault/exit；按 `max_restarts` + `backoff` 重建 init；否则长期 `Recv` 睡眠。

不做：不解析 ELF 之外的格式，不持有 VirtIO/FAT32，不提供 Map/WriteMemory 服务（这些是内核 `Runtime` 过渡能力，阶段 E 删除）。

### 3.2 init（service manager）

实现约束：init 的 supervisor 线程是唯一的 `control_ep` 接收者，且不得对它监督的服务做阻塞 `Call`；需要同步 `Call` 的 client/logger 放在另一个线程。独立 fault-handler 线程与它所需的 TCB/VSpace/CSpace 分离见 [独立 fault-handler 线程与线程模型](fault-handler.md)；组级销毁与组内线程的 fault 监督见 [进程/线程组生命周期与组内故障监督](thread-group.md)。

做：

- 持有 untyped 预算（子 untyped）、设备 untyped、ROM Frame cap、自身 TCB。
- 启动时读取 archive 里的 `init.cfg`（决策 2：文本配置）。
- 按依赖拓扑顺序启动服务。对每个服务：
  1. 从 ROM 读 ELF，用 ELF loader 建独立 VSpace/CSpace/TCB；
  2. 从自己的 untyped 切出一块**子 untyped** 作为该服务预算（决策 4）；
  3. 从子 untyped retype 该服务的初始对象；
  4. 给服务 mint 自己 `control_ep` 的 badged cap，放入服务 CNode 固定槽位，并把该槽号写进 TCB 的 `fault_ep`（见 7.7：fault_ep 是服务 CSpace 中的槽号，故障时内核在**被监督线程自己的 CSpace** 解析，与 seL4 non-MCS 一致）；
  5. 授予：初始对象 caps、`command_ep`、依赖服务 endpoint caps、按需设备 untyped；
  6. `resume`。
- 主循环 `ReplyRecv(control_ep)`：badge 识别服务，label 区分 READY/REPORT/EXIT/STOP 回复/Fault。
- 重启策略：`never / on-failure / always` + `max_restarts` + `window` + `backoff`。
- 依赖处理：服务 READY 前不启动依赖者；依赖崩溃时按策略通知或重启依赖者。

### 3.3 服务（console / block / fs）

- 独立进程，最小 capability，无 untyped 之外的特权。
- 启动后 `Call(control_ep, READY)`；运行中 `Call(control_ep, REPORT, ...)`；正常退出 `Call(control_ep, EXIT, code)`；崩溃由内核把 fault 投递到 `control_ep`。
- 监听 `command_ep` 上的 `STOP`/`PING`（决策 5：先实现握手）；服务用 `Reply` 回应。

### 3.4 appmgr（独立应用管理器，决策 6）

- 作为 init 的一个普通服务启动，`depends = [fs]`。
- 持有应用所需的 untyped 预算和 `fs-server` 的 client cap。
- 从 `fs-server` 读取应用 ELF，用同一套 ELF loader 创建应用进程。
- 对应用执行与 init 类似的生命周期管理（READY/report/重启），但策略属于应用域。
- init 不直接加载应用；应用崩溃由 appmgr 处理，appmgr 崩溃由 init 处理。

### 3.5 mysh（交互式 shell）

- 作为 init 的一个普通服务启动，`depends = [fs]`，`restart = never`（一次性演示）。
- 直接使用 `fs` 协议（`OPEN/READ/CLOSE/READDIR`）与自己的共享缓冲，不经过 appmgr。
- 提示符 `[rstiny ~]$: `，命令从 console 服务读取（`CONSOLE_READ` 轮询）；支持 `ls`、`cat <file>`、`./hello`、`help`、`exit`，以及退格/Ctrl-C/Ctrl-D。
- console 服务目前是轮询 RX（无 RX 中断），shell 在无输入时 `sleep` 后重试，不忙等。
- `./hello` 用与 appmgr 相同的 ELF loader 从磁盘装载 `HELLO.ELF`，并按 `libs/server` 协议监督它（`READY` 回执、`EXIT` 回收）。

## 4. 目录与构建

```text
projects/apps/userboot/          # 由 fatboot 改名；去掉 hello 嵌入
projects/apps/init/              # service manager
projects/apps/console/           # 用户态串口服务
projects/apps/block/             # VirtIO MMIO 块驱动（阶段 D）
projects/apps/fs/                # FAT32 只读（阶段 D）
projects/apps/appmgr/            # 应用管理器（阶段 D）
projects/apps/mysh/              # 脚本驱动 shell（阶段 D，D4）
projects/libs/server/            # 服务运行时：注册、report、fault、panic→EXIT
projects/libs/initcfg/           # init.cfg 解析器（no_std + alloc，可宿主测试）
projects/libs/newc/              # newc CPIO 只读解析（bootloader 与 userland 共用）
projects/libs/protocol/          # console/block/fs wire 定义（abi 依赖，无内核依赖）
projects/libs/virtio/            # VirtIO MMIO 轮询驱动（阶段 D）
projects/libs/fatfs/             # FAT32 只读（阶段 D）
```

构建改动：

- `Makefile`：`fatboot` target → `userboot`；新增 `init`、`console`、`block`、`fs`、`appmgr`、`mysh`。
- 应用磁盘由 `make disk` 生成：`APPS.CFG` + `HELLO.ELF`。缺省 `APPS.CFG` **为空**（appmgr 不自动启动任何应用，由 shell 的 `./hello` 按需运行）；D3/D5 验收用 `apps/APPS-hello.CFG`（`make APPS_CFG=...`）。
- `tools/build_image.py`：CPIO 从 `kernel + dtb + rootserver` 扩展为 `kernel.elf + kernel.dtb + userboot + init + services + init.cfg`；前三个文件名与顺序保持 bootloader 现有校验，其后为模块文件。
- `tools/build_app.py` 增加多应用构建入口；各应用共用 LLD 默认布局，段保持页不重叠（ELF loader 依赖该性质，见 13.4）。
- 文档、`tools/check_*.py`、README 中的 fatboot 引用同步改名。

## 5. BootInfo v6 与 boot module archive

### 5.1 loader 交接扩展

当前 bootloader 把 `kernel.elf/kernel.dtb/rootserver` 链接进自身 `.boot_archive` 段，拷贝三个镜像到 RAM 后用六寄存器交接（`docs/boot.md`）。userboot 需要读到**整个** archive（含 init/服务/init.cfg），因此：

1. bootloader 额外把 archive 原始字节按页对齐拷贝到一个空闲物理区间（选址复用 `memory::placement`，避开内核/DTB/root/自身）。
2. 交接寄存器扩展为八个：`x6` = archive 物理起点，`x7` = archive 字节数（决策 8）。这是对 seL4 elfloader 六寄存器交接的本机扩展；seL4 本身通过 bootinfo 结构传递同类信息，本项目选择最小改动。
3. 内核 `boot::information()` 接收并校验 `x6/x7`；`boot_regions` 把 archive 区间加入保留区，不产生 Untyped。
4. 内核把 archive 页发布为 boot Frame cap（复用 `Frame::take_boot` 的 boot 帧路径；`prepare_boot` 扩展为可接管两个区间：root image 与 archive，各自受 1024 页的 boot 帧位图上限约束，即 archive 目标 ≤ 1 MiB、上限 4 MiB）。

### 5.2 BootInfo v6

`ABI_VERSION` 5 → 6；头部保持 128 字节不变（`untyped_start`/`untyped_count`/`reserved[6]` 已在 v5 固定）。扩展记录沿用 id/len 头：

| id | 记录 | 布局 |
| --- | --- | --- |
| 6 | FDT | 现有 |
| 7 | Untyped 列表 | 现有 |
| 8 | boot module archive | `paddr: u64, size: u64, frame_start: u64, frame_count: u64` |

- `paddr/size`：archive 的物理范围（信息用；访问走 Frame cap）。
- `frame_start`：userboot CNode 中连续 `frame_count` 个 archive Frame cap 的起始槽号（固定 12，见 6.1）。
- `InitialTaskLayout::new` 的 `extra_size` 预算增加 `2 * sizeof(BootInfoHeader) + 32`（一条定长记录），`MAX_UNTYPED_REGIONS` 不变。
- 运行库（`rstiny-runtime`）校验 version == 6 后发布 `BootInfo::boot_modules() -> Option<&BootModules>`；version < 6 的旧镜像直接启动失败，不提供降级路径（root 镜像与内核同仓库同步构建）。

archive 内容（newc，名字即协议）：

```text
kernel.elf        # bootloader 直接消费；仍在 archive 中，userland 忽略
kernel.dtb        # bootloader 直接消费
userboot          # rootserver，bootloader 直接装载
init.elf
console.elf
block.elf
fs.elf
appmgr.elf
init.cfg
```

userboot 解析 archive 定位 `init.elf`；init 解析 archive 定位各服务 ELF 和 `init.cfg`。解析器从 `bootloader/src/image/archive/newc.rs` 抽出共享 crate `projects/libs/newc`（与 `rstiny-elf` 由 bootloader/userland 共用同一模式）：有界读取、无堆、拒绝内部 NUL 文件名与非法字段；userland 允许任意文件名集合（只要求非空且不重复），不再固定三个文件的顺序校验。

## 6. 能力与资源模型

| 边界 | 授予 | 保留 |
| --- | --- | --- |
| userboot → init | 子 untyped（init 预算）、全部设备 untyped、ROM Frame cap、`control_ep` badged cap、SpawnInfo 页 | bootstrap reserve untyped、control_ep 对象母 cap、monitor 栈 |
| init → service | 初始对象 caps（VSpace/CSpace/TCB/frames）、`control_ep` badged cap、`command_ep`、依赖 endpoint、按需设备 untyped | init 的 untyped 池、其他服务 cap |

两条硬性设计原则：

1. **endpoint 由监督者拥有，服务只拿 badged cap。** 服务重启时 endpoint 对象不销毁，依赖者 cap 不失效；重启后的服务重新 `Call` 即可。
2. **每个服务一个子 untyped。** 服务崩溃后，init 对**子 untyped 的 cap**（init 自己持有的那份）执行 `CNode_Revoke`：内核 finalise 该区间全部子对象并把子 untyped 的 watermark 重置为 0（见第 8 节），init 保留该 cap 并在原预算内重建对象——重启不泄漏、预算不回退。父区间（init 预算）的 watermark 不回退，切出子 untyped 的那部分字节在 init 生命周期内永久占用。

### 6.1 初始 CSpace 布局

所有进程共用 `INIT_TCB=1 / INIT_CNODE=2 / INIT_VSPACE=3 / INIT_ASID_POOL=6 / INIT_IPC_BUFFER=10`（`projects/libs/abi/src/object.rs` 现值）。在此之上固定：

| 槽 | userboot（root） | init | 服务 / 应用 |
| --- | --- | --- | --- |
| 11 | — | `control_ep` badged cap（fault_ep 槽号） | `control_ep` badged cap（fault_ep 槽号） |
| 512..512+N | archive Frame（`frame_start = 512`，N 在 BootInfo 记录 8；窗口必须避开固定槽与 Untyped 区） | archive Frame（ROM，userboot Copy 而来） | `command_ep` |
| 其余 11..19 | 保留给未来初始对象 | ROM 续 | 服务相关 endpoint（console/block/fs client，按 SpawnInfo） |
| 17 | `INIT_RUNTIME`（过渡） | `INIT_RUNTIME`（过渡） | `INIT_RUNTIME`（过渡：Current/Sleep/Clock；阶段 E 缩减，见决策 12） |
| 32.. | `INIT_UNTYPED` 起 boot Untyped | `INIT_UNTYPED` = init 预算子 untyped | `INIT_UNTYPED` = 服务子 untyped（仅当配置授予） |
| 33.. | — | 设备 untyped 槽位按 SpawnInfo | 设备 untyped（仅对应驱动） |

- ASIDPool：当前内核只有一个 `Object::AsidPool`。init/服务收到它的 Copy（逻辑分配，硬件仍 ASID 0 + 全量 TLB 失效，见 `docs/sel4-abi.md` 差异项）。
- 槽位是 ABI 的一部分：监督者把实际布局写进 SpawnInfo 页（6.2），程序用 SpawnInfo 而不是硬编码槽号（`INIT_*` 仍为回退值）。

### 6.2 SpawnInfo 参数页

`elf::spawn` 目前传 `x0 = 0`。改为监督者创建一页 RW Frame 映入子进程，`Task::start`/`write_initial_registers` 的 argument 指向它：

```rust
#[repr(C)]
pub struct SpawnInfo {
    pub magic: u64,          // 'RSTI'
    pub version: u64,        // 1
    pub control_ep: u64,     // 槽 11
    pub command_ep: u64,     // 槽 12；userboot 起 init 时为 0（无 command_ep）
    pub untyped: u64,        // 槽 32；0 = 未授予
    pub rom_start: u64,      // ROM Frame 首槽；0 = 无
    pub rom_count: u64,
    pub rom_paddr: u64,      // archive 物理起点（对齐与校验用）
    pub rom_size: u64,
    pub extra: [u64; 8],     // 服务相关 endpoint 槽号 / argv，逐协议定义
}
```

`projects/libs/server` 在入口把 SpawnInfo 解析为 `Service` 上下文；应用/服务代码不接触裸页。

### 6.3 子 untyped 切分（内核语义）

`Retype` 目标类型取 `ObjectType::Untyped = 0`（编号已在 ABI 保留），即 seL4 的 `Untyped_Retype(Untyped → Untyped)`：

- `size_bits = k`，`12 <= k <= min(30, 父 size_bits)`；`count` 1..=32，逐个按 `(2^k, 2^k)` 对齐从父 watermark 切分（走现有 `Untyped::fits` 精确模拟 + 一次性提交，失败不推进 watermark）。
- 子 untyped 继承父 `is_device`。普通父区间切出的子区间不做整体预清零；保密性由既有不变式保证：从子区间 retype 出的对象在分配时清零（`new_untyped_frame` 现状），`Revoke` 归还时整段清零（第 8 节）。
- 子 untyped 对象记录 `ObjectOwner { untyped: 父, offset, size: 2^k }`，因此 `ObjectTable::children()` 天然给出派生树（第一版 O(对象数) 扫描，与现有实现一致）。
- `object_allocation()` 增加 `Untyped` 分支 `(1<<k, 1<<k)`；设备 Untyped **不**允许切分（决策 11，设备区间整段授予驱动）。

### 6.4 `CNode_Mint` badge

控制协议依赖 badged cap，而当前 Mint 拒绝 `capData != 0`（`kernel/src/object/cnode.rs:131`）。本设计补齐 seL4 语义：

- 仅对 Endpoint/Notification cap：`badge_new = capData & badge_old`；非零结果要求源 cap 持有 `RIGHTS_GRANT`。
- CNode/Frame/TCB 等 cap 仍拒绝 `capData != 0`（本项目 CSpace 无 guard，维持现状）。
- badge 交付：Endpoint 收到消息时把发送者 cap 的 badge 放进接收者 x0（见 7.3）；badge 0 表示无 badge，因此监督者分配 `badge = 服务序号 + 1`。

## 7. 内核机制：Endpoint、Notification 与 fault endpoint

这是阶段 A 的核心，也是本设计与现状差距最大的一块。现状：`api/dispatch.rs` 对 `Send/NBSend/Recv/Reply/ReplyRecv/NBRecv` 一律 `Disposition::Fault`；`Object` 无 Endpoint/Notification；`TcbConfigure` 要求 `fault_ep = 0`。ABI 层面编号已就位：`ObjectType::Endpoint = 2 / Notification = 3`，syscall 号 `Send = -3 ... NBRecv = -8`（`projects/libs/abi/src/syscall.rs`）。

### 7.1 对象与权利映射

| 项 | 取值 |
| --- | --- |
| 对象负载 | `Object::Endpoint { state, queue }`、`Object::Notification { bits, queue }`，负载在对象表（与 TCB/CNode 同类元数据对象） |
| 计费 | 名义预算 Endpoint/Notification 各 64 字节、64 对齐（沿用 TCB 1024B / CNode 8B/槽 的"元数据对象记账"约定），推进父 Untyped watermark |
| Endpoint 权利 | `RIGHTS_WRITE` = send（canWrite）、`RIGHTS_READ` = recv（canRead）、`RIGHTS_GRANT` = 传递 cap（canGrant）、`RIGHTS_GRANT_REPLY` = 允许对方回复（canGrantReply）——与 seL4 四位一一对应 |
| Notification 权利 | `RIGHTS_WRITE` = signal、`RIGHTS_READ` = wait |
| fault ep | TCB 保存 `fault_ep: u64`（CPtr）。**配置时**：`TcbConfigure` 第 0 参数为槽号，内核只做范围检查（0..65536），不解析；**故障时**：内核在该故障线程自己的 CSpace 解析该槽号，必须得到 Endpoint cap（seL4 non-MCS `tcbFaultHandler` 语义） |

### 7.2 阻塞模型与现有调度器的集成

现有机制已经具备阻塞所需的全部底座：每任务内核 continuation、`park(Disposition)` 保留 Rust 调用链、`finish()` 的"写 completion + ready"唤醒路径（`kernel/src/task/scheduler.rs:273`、`:155`）。IPC 沿用同一模式：

- 新增 `Disposition` 变体：`BlockSend { ep, call }`、`BlockRecv { ep }`、`BlockFault { ep }`（不再复用 `Disposition::Fault`，后者保持"不可恢复 → 终止"语义）。
- 新增任务状态常量（`projects/libs/abi`）：`TASK_BLOCKED_SEND = 8`、`TASK_BLOCKED_RECV = 9`、`TASK_BLOCKED_REPLY = 10`、`TASK_BLOCKED_FAULT = 11`。它们是**可恢复**状态，区别于终态 `TASK_FAULTED`。
- Endpoint/Notification 的等待队列是 TCB 内嵌的 FIFO 链（`Task` 增加 `ep_prev/ep_next: u64`，0 表示不在队列；对象保存 head/tail 任务 id）。节点不堆分配，与 `RunQueue` 同法。
- 系统调用路径：先完成全部校验和状态提交（谁阻塞、消息是否可拷贝、目标槽是否为空），**不持有 store/scheduler 借用**再 `park(...)`；被对端唤醒时 `park()` 返回，syscall 处理函数读回 completion 并把消息写入本任务寄存器/IPC buffer，随后按普通 `Resume` 返回用户态。
- 唤醒方（投递者）做三件事：把消息写进接收者已保存的 TrapFrame（x0 = badge、x1 = MessageInfo、x2..x5 = 前 4 个 MR）和接收者 IPC buffer；设置 `tasks[slot].completion`；`ready(slot)`。这要求给 `Task.execution` 的已保存 `UserContext` 增加受控修改入口（现在只在创建时写一次）。
- 阻塞任务不在就绪队列；`suspend()` 对阻塞任务保留阻塞原因（沿用 `suspended_from`），`resume()` 后回到原等待队列。

### 7.3 Endpoint 状态机与 Call/Reply

```text
Endpoint.state: Idle | Send(head,tail) | Recv(head,tail)

Send(ep):   cap 需 RIGHTS_WRITE。
            Recv 队列非空 → 弹出接收者，交付消息，发送者 Resume。
            否则入 Send 队列，BlockSend。
Recv(ep):   cap 需 RIGHTS_READ。
            Send 队列非空 → 弹出发送者，交付，接收者拿到 badge 后 Resume。
            否则入 Recv 队列，BlockRecv。
Call(ep):   cap 需 RIGHTS_WRITE（seL4: Write + 目标可回复取决于本 cap 的 GRANT_REPLY）。
            = Send + 记录回复关系 + BlockReply。
Reply:      消费本任务的一次性 reply 关系，向对方交付回复消息并唤醒。
            无 reply 关系 → IllegalOperation（seL4 同）。
ReplyRecv:  = Reply（若存在）+ Recv；Reply 失败则整个调用失败，不进入接收。
NBSend:     有接收者才交付，否则无操作成功返回。
NBRecv:     有发送者才接收，否则立即返回 badge=0、length=0。
```

一次性 reply 关系（决策 13）：接收任务在收到 `Call` 交付或 fault 交付时记录 `caller: Option<Caller>`，`Caller::Call(task)`（普通调用方）或 `Caller::Fault(task)`（故障线程）。未回复前不可覆盖；`Reply` 消费后清空。回复时按 kind 区分：

- `Caller::Call`：写回复 MessageInfo（label + MR）到对方 IPC buffer/寄存器，唤醒。
- `Caller::Fault`：**不写 MR**，把故障线程的 restart PC 写回其 ELR 后唤醒（对应 seL4 `handleFaultReply` 里 VMFault reply 只恢复执行；寄存器修复由监督者先 `TCB_WriteRegisters` 完成，见 7.7）。

与 seL4 的差异：seL4 non-MCS 的 reply 是 TCB 内的隐式 caller cap（可复制到 CSpace 的 `seL4_Caller` 槽）；本项目不把 reply 暴露为可转移 cap，只保留隐式一次性语义。嵌套调用、显式保存 reply 延后。

### 7.4 消息传递

- MessageInfo 与 IPC buffer 布局完全沿用现有 seL4 格式（`projects/libs/abi/src/message.rs`：tag 0、msg[120] 于 8、caps/badges 于 976、接收描述于 1000..1024）。
- 发送侧：MR ≤ 4 走寄存器 x2..x5；`length > 4` 或 `extraCaps > 0` 时其余 MR 从发送者 IPC buffer 读取。接收侧对称：前 4 个 MR 写 x2..x5，全部 MR 写接收者 buffer（`length` 上限 120，越界即 `TruncatedMessage`，先校验后提交）。
- badge 交付在 x0（seL4 AArch64 badge 寄存器约定）；接收者的 x1 为接收 tag：`label = 发送者 label`、`extraCaps = 实收 cap 数`、`capsUnwrapped = 0`、`length = 发送 length`。
- 两个任务都必须有已配置的 IPC buffer（`Task.ipc_buffer` 非 0 且映射有效）才允许 `length > 4` 或带 cap 的消息；纯寄存器消息不要求 buffer。
- 跨地址空间复制用现成原语：`AddressSpace::read/write`（`memory/space.rs`，`Runtime::ReadMemory/WriteMemory` 同路径），每次 ≤ 1024 字节并做边界检查。
- **协议 label 空间**：seL4 中 fault 消息与普通消息共用 MessageInfo.label，且内核按对象类型分配连续 invocation label 段。这里照搬该纪律：fault 占用 `0..=4`（见 7.7），每个用户协议占一个 256 宽的段（console `0x100`、control `0x200`、internal `0x300`、block `0x400`、fs `0x500`），内核 Runtime 扩展占 `0x1000` 段。段之间不得重叠——否则同端点处理两个协议时会互相遮蔽（原提纲 `READY = 1` 与 `CapFault = 1`、以及 fs `STAT` 与 control `STOP` 都属这类冲突）。`projects/libs/protocol` 用编译期断言加宿主测试 `tests/segments.rs` 守住。

### 7.5 cap 传递

- 发送者 buffer 的 `caps_or_badges[0..extraCaps]` 是发送者 CSpace 中的槽号；每个被传 cap 要求 `RIGHTS_GRANT`，解析失败或无权即整个发送失败（无半次交付）。
- 接收者 buffer 的 `receive_cnode/receive_index/receive_depth` 指定落点：`receive_cnode` 在接收者 CSpace 中解析为 CNode cap（要求 `RIGHTS_GRANT`）；本项目 CSpace 单层，`receive_depth` 必须 = 64。目标槽必须为空，逐槽校验后再写。
- 交付的 cap 通过 `insert_cap` 建立，派生父为源 cap serial（沿用 `MAX_DERIVATIONS` 记账）；映射记录不随消息转移。
- `capsUnwrapped` 恒为 0：v1 不实现 endpoint unwrap（决策 10）。消息中传递 endpoint cap 时接收者得到独立派生 cap，badge 不变。

### 7.6 Notification

- `Signal`（`Send` 到 Notification cap，`RIGHTS_WRITE`）：有等待者 → 取 badge|word 唤醒一个；无等待者 → `bits |= (badge | word)` 合并。
- `Wait`（`Recv`，`RIGHTS_READ`）：`bits != 0` → 取走并清零，以 badge 返回；否则阻塞。
- `NBRecv`：非阻塞轮询版本。
- 不做绑定 TCB（`TCB_BindNotification`，仍延后）；IRQ 投递已落地：init 按服务授予 `IRQHandler`（与设备 Untyped 同副本模式，teardown 时对 master `Clear`）并经 `SpawnInfo::extra[IRQ_SLOT]` 下发（[irq.md](irq.md) §8/§13）。多次 Signal 合并语义要求驱动读设备直到清空事件源，沿用 seL4 语义。

### 7.7 fault endpoint 与故障消息

投递流程（替换 `api/faults.rs::handle_user_fault` 的"记录 + 终止"）：

1. 内核在故障线程的 CSpace 解析 `TCB.fault_ep` 槽号。
2. 解析结果是 Endpoint cap → 构造 fault 消息（badge = 该 cap 的 badge，label = fault 类型），以"故障发送者"身份走 Endpoint send（接收者可在等待，也可能把内核排入 Send 队列）。故障线程置 `TASK_BLOCKED_FAULT`，其 restart PC（`elr`）保存于 TCB。
3. 解析失败或不是 Endpoint → 双重故障：保持现状行为（记录 `LAST_FAULT`、终止该任务为 `TASK_FAULTED`，不投递）。
4. 监督者 `Recv` 到 fault 消息后：修复（`Frame_Map` 补页、`TCB_WriteRegisters` 改 PC/SP，允许作用于 `TASK_BLOCKED_FAULT` 任务，见 7.8），然后 `Reply` → 线程从 restart PC 恢复；或 `TCB_Suspend`/重启策略处置。

故障消息格式（label 取值按 libsel4 faults.xml 在 non-hyp AArch64 配置下的顺序；本项目为子集，MR 布局在 `projects/libs/abi` 固化）：

| label | 类型 | MR |
| --- | --- | --- |
| 0 | NullFault（保留） | — |
| 1 | CapFault（保留，v1 不产生） | 0 IP, 1 CPtr, 2 inRecvPhase |
| 2 | UnknownSyscall | 0 restart IP, 1 SP, 2 syscall 号 |
| 3 | UserException（FP/SIMD 陷入） | 0 restart IP, 1 ESR |
| 4 | VMFault | 0 restart IP, 1 FAR, 2 指令/数据（PrefetchFault）, 3 FSR(ESR) |

VMFault 的四个 MR 与 seL4 ARM `Arch_setMRs_fault` 完全一致（`seL4_VMFault_IP/Addr/PrefetchFault/FSR`）。UnknownSyscall/UserException 是 seL4 对应消息的子集（seL4 附带更多寄存器字），扩展时只增不改。

### 7.8 生命周期、等待者清理与 finalisation

- `TcbConfigure` 放开 `fault_ep`（第 0 参数为槽号，允许 0 = 无处理者）；仍校验 `cspaceData/vspaceData = 0` 与 IPC 对齐（现状）。
- `TCB_WriteRegisters`/`Runtime::Map` 类操作的目标扩展到 `TASK_BLOCKED_*` 状态（监督者拥有被阻塞线程）；其余状态沿用 `editable()` 守卫。
- 对象 finalisation 清单（`cnode.rs::finalise_untyped` / `delete`）新增：
  - **Endpoint**：取消队列中全部等待者——被取消的发送/接收者置 `TASK_SUSPENDED`（保留阻塞原因），其 fault_ep 若可解析则补发 CapFault；不再有半挂起的 queue 节点。
  - **Notification**：同样取消等待者（置 Suspended）。
  - **Untyped（子）**：递归 finalise 自己的子对象后再被父区间回收（现状 `finalise_untyped` 只处理一层，必须改为深度优先）。
- 删除一个正在被消息引用的 cap 与正在进行的 IPC 并发不存在：单核、IRQ 屏蔽、syscall 内串行，"先校验后提交"原则覆盖。

### 7.9 与 seL4 的对应与差异

| 项 | seL4 non-MCS | 本设计 |
| --- | --- | --- |
| 状态机/队列 | endpoint 内嵌 TCB 队列、阻塞状态机 | 同构（对象表负载 + TCB 内嵌链） |
| badge | badge 寄存器交付，Mint 做 badge AND | 相同（x0 交付） |
| fault ep | `tcbFaultHandler` 存 CPtr，故障时在故障线程 CSpace 解析 | 相同 |
| fault 消息 | faults.xml 类型 + 全寄存器 MR | label 一致；MR 为子集（VMFault 四 MR 一致） |
| reply | TCB 隐式 caller cap，可转存 | 隐式一次性关系，不暴露 cap |
| cap 传递 | Grant 检查 + unwrap | Grant 检查；无 unwrap |
| fastpath | 有 | 无（慢路径优先正确性，阶段 F 再测） |
| 绑定 Notification / 超时 IPC / MCS | 有/有/MCS | 无 |

## 8. 内核机制：Untyped 切分

阶段 A 第二个硬前置。现状 `retype` 只接受 `Tcb/CNode/VSpace/SmallPage/PageTable`（`kernel/src/object/invoke.rs:94`），语义细化见 6.3。实现落点：

- `invoke.rs::retype`：类型白名单加 `ObjectType::Untyped`；`size_bits` 校验放宽为该类型的规则；`object_allocation()` 加 Untyped 分支；子对象构造 `Object::Untyped(Untyped::new(phys, k, is_device))` + `ObjectOwner`。
- `cnode.rs::finalise_untyped`：改为深度优先递归（子 untyped 先 finalise 自己的子对象）；Endpoint/Notification 走 7.8 的 finalise。
- `Revoke(子 untyped cap)` 复用现有路径：收集后代 → 解映射/删 cap → finalise → `reset()`。**reset 作用于被 Revoke 的那个 untyped**；父区间 watermark 不动（seL4 `resetUntypedCap` 同语义）。

服务重启的完整回收链：

```text
init 持有服务子 untyped cap（永不删除）
服务崩溃 → Revoke(子 untyped cap)
  → 该区间全部对象 finalise（映射解除、TLB 失效、TCB 出队、endpoint 取消等待者）
  → 非设备区间清零，子 untyped watermark = 0
init 用同一 cap 重建服务初始对象 → 预算不泄漏
```

## 9. 文本配置 `init.cfg`

行式语法，无嵌套，注释 `#`：

```text
# init.cfg
service console {
    elf = "console.elf"
    restart = always
    max_restarts = 5
    window_ms = 60000
    backoff_ms = 100
}

service block {
    elf = "block.elf"
    depends = console
    restart = on-failure
    device = virtio-mmio-0
    budget = 2M
}

service fs {
    elf = "fs.elf"
    depends = block
    restart = on-failure
    budget = 1M
}

service appmgr {
    elf = "appmgr.elf"
    depends = fs
    restart = on-failure
    budget = 2M
}

# mysh: script-driven shell (apps/SH.CFG on the disk). Lists the disk, prints
# files and executes ./hello by loading HELLO.ELF from the disk.
service mysh {
    elf = "mysh.elf"
    depends = fs
    restart = never
    budget = 2M
}
```

键：

| 键 | 类型 | 默认 | 说明 |
| --- | --- | --- | --- |
| `elf` | string | 必填 | archive 中的文件名 |
| `argv` | string list | 空 | 传给进程的 argv（不含 argv0），经 SpawnInfo `extra` 传递 |
| `depends` | name list | 空 | 依赖的服务名 |
| `restart` | enum | `on-failure` | `never` / `on-failure` / `always` |
| `max_restarts` | u32 | 5 | 观察窗口内上限 |
| `window_ms` | u32 | 60000 | 观察窗口 |
| `backoff_ms` | u32 | 100 | 首次退避，指数增长 |
| `budget` | size | 1M | 子 untyped 大小，2 的幂，12..=30 位 |
| `device` | string list | 空 | 要授予的设备 Untyped 名 |
| `critical` | bool | false | 关键服务，失败时上报并停止依赖者 |

校验规则（`projects/libs/initcfg`，宿主单元测试覆盖）：名字唯一、`elf` 在 archive 中存在、依赖无环、`budget` 是 2 的幂且各服务预算之和 ≤ init 自身子 untyped 大小、`device` 名在 SpawnInfo 的设备表中存在。解析失败：init 记录错误并拒绝启动该系统（`EXIT(1)` 交给 userboot），不进入部分启动状态。

## 10. 控制协议（共享 `control_ep` + badge）

一个 `control_ep`，badge = service id（从 1 起），label 区分消息类型（决策 3）。label 空间避让 fault 段（7.4）：

| label | 方向 | 语义 | MR |
| --- | --- | --- | --- |
| `0x200 READY` | service → init | 启动完成 | 无 |
| `0x201 REPORT` | service → init | 状态/指标 | mr0 = status code，mr1.. 自定义 |
| `0x202 EXIT` | service → init | 正常退出 | mr0 = exit code |
| `0x203 STOP_ACK` | service → init | 停止握手确认（`Reply` 回执） | 无 |
| `0x204 STOP` | init → service | 请求优雅停止 | mr0 = 超时 ms |
| `0x205 PING` | init → service | 健康检查 | 无 |
| `0x206 DEPENDENCY_LOST` | init → service | 依赖者失效，自行退出或降级 | mr0 = 依赖服务 badge |
| `0..=4` | kernel → init | fault 消息（7.7） | 按 fault 类型 |

`control` 独占 `0x200` 段；console/block/fs 各自的段见 [disk-driver.md](disk-driver.md) §8 与 `projects/libs/protocol`。同端点收到两个协议时不再互相遮蔽。

- service 侧：`Call(control_ep, READY)`，init `Reply` 确认；`REPORT`/`EXIT` 同理。
- init 侧：`ReplyRecv(control_ep)` 同时收 report 和 fault，靠 label 区分、badge 识别来源（badge 0 不可能出现：badge 从 1 分配）。
- `command_ep` 是每个服务单独的对象，用于 init → service 的 `STOP`/`PING`；服务 `NBRecv(command_ep)` 轮询或阻塞 `Recv` 后用 `Reply` 回 `STOP_ACK`。
- STOP 超时判断需要时钟：阶段 F 前 init 用 `Runtime::Clock`（决策 12），之后换用户态定时器 Notification。

## 11. STOP 握手（决策 5）

```text
init                                  service
  |-- Call(command_ep, STOP, timeout) -->|
  |                                      | 停止接收新请求、flush、持久化
  |<-- Reply(command_ep, STOP_ACK) ------|
  |                                      | Call(control_ep, EXIT, code)
  |<-- Recv(control_ep, EXIT) -----------|
  |  TCB_Suspend / CNode_Revoke(子untyped)
```

超时未 `STOP_ACK` 或未 `EXIT`：init 直接 `TCB_Suspend` + `Revoke`，记录 `forced`。握手让服务有机会释放设备、停止 DMA；对无 IOMMU 的总线主控设备尤其重要。

## 12. 重启状态机与策略

```text
Starting --READY--> Running
Running  --Fault/EXIT--> Terminated
Running  --STOP/STOP_ACK--> Stopping --EXIT--> Terminated
Terminated --策略允许--> Starting   (backoff, 未超阈值)
Terminated --策略拒绝--> Failed
```

重启步骤：

1. 标记服务 `not-ready`；对依赖者发 `DEPENDENCY_LOST`（按策略）。
2. 对服务的子 untyped cap 执行 `CNode_Revoke`，回收全部对象并重置其 watermark（见第 8 节）；endpoint 对象不销毁。
3. 从 ROM 重新装载 ELF，重新 retype 初始对象、重授 caps，`resume`。
4. 等 `READY`；按依赖策略恢复依赖者。

Restart storm guard：`window_ms` 内重启次数超过 `max_restarts` → 标记 `Failed`，停止重启，向 userboot report。

依赖语义：

- `depends` 是启动顺序约束：被依赖者 READY 后才启动。
- 被依赖者进入 `Terminated/Failed` 时，依赖者收到 `DEPENDENCY_LOST`，自行决定退出或降级。
- `critical = true` 的服务失败：init 停止其依赖者并上报，不自动重启整棵子树。

## 13. init 内部结构

### 13.1 模块划分

```text
projects/apps/init/src/
  main.rs          # 入口：读 SpawnInfo/ROM → 解析 cfg → 起服务 → 主循环
  services.rs      # 服务表：状态机（第 12 节）、重启计数、依赖闭包
  spawn.rs         # 逐服务 spawn（13.3）
```

`projects/libs/initcfg` 只做纯解析（输入 `&str`，输出服务描述向量），可被宿主测试与 init 同时使用；不依赖内核 ABI。

### 13.2 主循环

```text
loop {
    (badge, label, mrs) = ReplyRecv(control_ep)     # 回复上一条 + 收下一条
    match label {
        READY  => 状态 Starting→Running；启动依赖它的下一批服务
        REPORT => 记录指标
        EXIT   => 策略判定：重启 or Failed or （STOP 流程）Terminated
        STOP_ACK => Stopping 推进
        fault(0..=4) => 记录 crash；Revoke 子 untyped；按策略重启服务线程
    }
}
```

init 阻塞在 `ReplyRecv` 时进入内核 idle（`root_idle` 的 WFI），不空转。

### 13.3 服务 spawn 序列（用户态，全部标准对象操作）

1. `retype(Untyped, size_bits = log2(budget))` 从 init 预算切子 untyped；cap 常驻 init CNode（key = 重启不泄漏，见第 8 节）。
2. 从子 untyped retype TCB/CNode/VSpace/PageTable/Frame；`ASIDPool_Assign`（用 init 持有的 ASIDPool Copy）。
3. ROM Frame cap 已在 init CNode：map 到 init 的 scratch VA，逐段拷入子进程页（与 `rstiny::elf::spawn` 相同的 alias 技法）；设备 untyped 按配置 Move 给驱动。
4. CNode_Copy/Mint：服务自身 TCB/CNode/VSpace/IPCFrame → 子 CNode 固定槽；`control_ep` mint badge；`command_ep`、依赖服务 endpoint Copy；子 untyped（若配置）Copy 给服务，init 保留母副本——重启时 `Revoke(母副本)` 即可连带删除服务副本、finalise 全部对象并重置区间（第 8 节）。
5. `TcbConfigure(cspace, vspace, ipc_frame, ipc_va, fault_ep = 槽 11)`；`WriteRegisters`（PC/SP/参数页）；`Resume`。
6. 失败回滚：`Revoke` + `Delete` 私有 untyped cap（`elf.rs` 现有模式）。

### 13.4 用户态基础设施改造

- **槽位分配**：`elf.rs` 现依赖 `Runtime::FindEmptySlot`。阶段 B 引入 `projects/libs/user::SlotAlloc`（初始化为 `32..65536` 空闲位图，分配/释放由调用者显式记账；监督者发给子进程的槽位从子 CNode 视角重新编号）。loader 与 init 都用它，`FindEmptySlot` 依赖从 loader 路径移除。
- **ELF loader 参数化**：`projects/libs/user/src/elf.rs::spawn` 拆为 `Loader { untyped, slots, scratch, rom }`，`spawn_spec(spec: &SpawnSpec)`；root 的便捷封装保留。前置校验沿用 `rstiny-elf`（静态、小端、AArch64、≤32 phdr、页对齐段、`filesz<=memsz`）。
- **scratch VA 策略**：每进程约定 `SCRATCH_VA = USER_ADDRESS_LIMIT - 2 * PAGE` 起的私有窗口（该区域不用于镜像/栈/IPC，`USER_ADDRESS_LIMIT = 128 MiB` 之内）；loader 内部先映射 PageTable 再复用。
- **段页不重叠**：loader 按页独立上权限，两个段共享一页会导致权限互相覆盖/映射失败。`tools/build_app.py` 的 LLD 默认布局已产生页对齐独立段；该前提写入 loader 文档并在 spawn 时校验（发现共享页直接拒绝）。
- **时间**：退避/窗口计时用 `Runtime::Clock`（毫秒），阶段 F 换 Notification 定时器。

## 14. libs/server 与服务协议

### 14.1 服务运行时

```rust
// projects/libs/server
pub struct Service { badge, control_ep, command_ep, /* SpawnInfo 派生 */ }
impl Service {
    pub fn init(&self) -> Result<(), Error>;          // Call(control_ep, READY)
    pub fn report(&self, status: u64, mrs: &[u64]);   // Call(control_ep, REPORT)
    pub fn exit(&self, code: u64) -> !;               // Call(control_ep, EXIT)
    pub fn poll_stop(&self) -> Option<u64>;           // NBRecv(command_ep) → STOP 超时
}
#[macro_export] macro_rules! log { ... }              // Call(console_ep, CONSOLE_WRITE)
```

- panic handler（`rstiny-runtime`）改为：尽力 `Call(control_ep, EXIT, 0xF...F)`，失败再 `suspend_self`——避免静默挂死；崩溃的最终事实仍由 fault 投递兜底。
- 服务主循环模型：`loop { poll_stop(); Recv(service_ep) or Wait(ntfn); ... }`。

### 14.2 wire 协议（`projects/libs/protocol`）

各服务 endpoint 独立、无 fault 投递，label 可从 1 起；每个协议常量 `PROTOCOL_VERSION`，绑定握手指令携带并校验。

console（`console_ep`）：

| label | 方向 | MR |
| --- | --- | --- |
| 1 `CONSOLE_BIND` | client → server | mr0 = 版本；回复 mr0 = 版本、mr1 = 最大内联长度 |
| 2 `CONSOLE_WRITE` | client → server | mr0 = 字节数 n（≤ mr1 上限），mr1.. 按 8 字节/MR 内联打包；回复 mr0 = 已写字节 |
| 3 `CONSOLE_READ` | client → server | 无；回复 mr0 = 是否有字节（1/0）、mr1 = 字节 |

内联上限 14 MR = 112 字节（`MAX_WRITE`），客户端分块。v1 无共享缓冲。RX 为轮询（无中断），客户端在空读后自行退避。

block（`block_ep`，阶段 D）：

| label | 方向 | MR / cap |
| --- | --- | --- |
| 1 `BLOCK_BIND` | client → server | mr0 = 版本；附共享 DMA Frame cap（Grant）；回复 mr0 = 版本、mr1 = 容量（扇区）、mr2 = 扇区大小 |
| 2 `BLOCK_READ` | client → server | mr0 = 起始 lba、mr1 = 扇区数；回复 mr0 = status、mr1 = 实读扇区数 |

DMA buffer 经 `BLOCK_BIND` 授予的共享 Frame 传递（client 映射 RW，server 映射 RW；单并发请求，轮询完成，阶段 F 换 IRQ Notification）。无 IOMMU：server 是受信任组件，能 DMA 到任意物理地址——该边界写入 `docs/microkernel-design.md` 已有结论，服务拆分不改变它。

fs（`fs_ep`，阶段 D）：

| label | 方向 | MR |
| --- | --- | --- |
| 1 `FS_BIND` | client → server | mr0 = 版本；回复 mr0 = 版本 |
| 2 `FS_OPEN` | client → server | mr0 = 路径字节数，mr1.. 内联路径；回复 mr0 = 句柄或错误 |
| 3 `FS_READ` | client → server | mr0 = 句柄、mr1 = 偏移、mr2 = 长度；回复 mr0 = status、mr1 = 实读长度（数据经 `FS_BIND` 附带的共享 Frame） |
| 4 `FS_SIZE` | client → server | mr0 = 句柄；回复 mr0 = 文件字节数 |
| 5 `FS_READDIR` | client → server | mr0 = 起始条目下标；回复 mr0 = status、mr1 = 写入条目数、mr2 = 下一下标（0 = 结束）；条目为共享 Frame 中的 `DirEntry` 数组 |

`DirEntry` 为 `{ name:[u8;12]; size:u32; is_dir:u32 }`，一页共享缓冲容纳 `DIR_ENTRIES_PER_PAGE` 条。appmgr 用 `FS_OPEN/FS_READ` 读应用 ELF 到自己授予的共享 Frame；`mysh` 额外用 `FS_READDIR` 列根目录。

## 15. appmgr 与应用生命周期

- `appmgr` 是 init 的普通服务，`depends = [fs]`。
- 应用来源：`fs-server` 的只读文件；应用清单文件（`APPS.CFG`，语法同 `init.cfg`）由 appmgr 从 fs 读取。缺省清单为空——开机不自动跑应用，`mysh` 的 `./hello` 才是运行入口；D3/D5 验收换用 `APPS-hello.CFG`。
- appmgr 负责：解析清单、切应用子 untyped、ELF 装载、READY/report、应用级重启策略。
- 应用与系统服务使用同一套 `libs/server` 协议，但控制端点是 appmgr 的 `control_ep`；应用不接触系统服务的 endpoint（console 例外，经 appmgr Copy）。
- init 不感知具体应用；只监督 appmgr。

## 15.1 mysh 与 `./hello`

- `mysh` 是 init 的普通服务，`depends = [fs]`，在 console 上开一个 REPL（`[rstiny ~]$: `）。
- 输入来自 `CONSOLE_READ`：console 服务轮询 PL011 RX FIFO，无字节就回空；shell 空读后 `sleep(5ms)`，不忙等。行编辑只做回显、退格和 Ctrl-C/Ctrl-D。
- `./hello` 复用 appmgr 的 loader：切一个子 untyped、填 `SpawnInfo`（`control_ep = 140`、`console_ep = 51`），并以子进程的 `control_ep` 为 fault/控制端点监督它。
- 关键差异：`hello` 是标准服务，装载后会 `Call(control_ep, READY)`，因此 shell 必须像 appmgr 一样 `Recv(control_ep)` 并 `Reply`，否则子进程停在 `BlockedSend`、`Wait` 永不返回。

## 16. 日志

- `console_server` READY 前：init/userboot 用内核 debug console（`debug_println!`，受 `LOG` 与 `FEATURE_DEBUG_CONSOLE` 控制）。
- READY 后：init 与服务的日志走 console 协议；`libs/server` 的 `log!` 内部 `Call(console_ep, CONSOLE_WRITE)`。
- 边界（明确化）：内核自身的 `log` 宏**始终**直写 PL011，不经过用户服务，也没有跨地址空间锁。debug 运行时内核日志与用户输出可能交错；生产/测试运行用 `LOG=off` 获得干净输出。`DebugPutChar` 在 console 服务可用后仅保留给 `kernel-test`。
- 服务 panic：见 14.1，先 EXIT 后 fault 兜底。
- 换行：`libs/server` 的 `logln!` 负责在行尾补 `\n`（`log!` 不补）；console 服务把 `\n` 翻译为 `\r\n` 再写 PL011，与内核 debug console 一致（终端需要 CR）。init 的 logger 线程写 console 时同样补 `\n`。

## 17. 调度与实时性假设

当前调度器是单 FIFO 就绪队列 + 10 ms 时间片轮转（`kernel/src/task/scheduler.rs`），无优先级。影响：

- init/服务阻塞在 `Recv` 时不占 CPU（进 idle WFI），监督循环本身没有开销。
- 服务与应用同优先级轮转：一个忙循环客户端会平分 CPU，console/block 服务不会饥饿，但没有时延保证。
- 存在优先级反转与饥饿；固定优先级（`TCBSetPriority`，seL4 label 7）列为阶段 F 候选，本设计不依赖它。

## 18. 分阶段实施

| 阶段 | 内容 | 前置 |
| --- | --- | --- |
| A | Endpoint、Notification、fault endpoint、Untyped 切分、用户态 slot/untyped 分配器、boot module archive（内核侧） | 现有对象模型 |
| B | `fatboot → userboot`；bootloader 透传 archive + x6/x7；userboot 从 archive 起 init；init 空转并 READY；userboot 监督 init | A |
| C | init 从 ROM 起 `console_server`；report；杀 console → init 重启 → console 恢复 | B |
| D | `block-server` + `fs-server` + `appmgr`；加磁盘；应用从 fs 加载 | C |

阶段 D 的磁盘分层、VirtIO MMIO 驱动、FAT32 解析、共享内存与验收见 [磁盘与 FAT32 用户态驱动设计](disk-driver.md)。
| E | 删除内核 `Runtime` 托管标签 / `managed_untyped` / `collect` 降级；userboot 变纯 monitor | D |
| F | report 服务化、依赖重启、backoff、restart storm guard、STOP 全面接入、优先级/IRQ Notification 候选项 | E |

阶段 A 是门槛：没有 Endpoint + fault endpoint，init 无法收 report、无法感知崩溃；没有 Untyped 切分，重启会泄漏。

文件级改动：

| 阶段 | 内核 | 用户态 | 工具/测试 |
| --- | --- | --- | --- |
| A | `abi`（状态常量、fault label、`ObjectType` 已就绪）；`object/{mod,invoke,cnode,untyped}.rs`（新对象、finalise 递归、retype 扩展）；新增 `object/{endpoint,notification}.rs`；`api/dispatch.rs`（六个 IPC syscall）；`api/faults.rs`（fault 投递）；`task/{scheduler,api}.rs`（阻塞状态、队列链、`Task.execution` 受控写、`editable()` 放宽） | `abi`（fault/状态常量）；`user::SlotAlloc`；`rstiny` retype Untyped/Endpoint 封装 | 新增 `tools/check_ipc.py`、`tools/check_fault_ep.py`；扩展 `check_untyped.py`（切分/递归 Revoke）；宿主 wire 测试 |
| B | `arch/kernel/boot.rs` + `memory/frame.rs`（archive 帧接管、保留区）；`boot.rs` 发布记录 8 + Frame cap | bootloader（archive 拷贝 + x6/x7）；`build_image.py`；`libs/newc`；`apps/userboot`；`apps/init`（空转 + READY）；`libs/runtime`（v6、boot_modules、SpawnInfo） | `check_bootloader.py` 扩展；新增 `check_userboot.py` |
| C | — | `apps/console`；`libs/server`；`libs/protocol`（console）；init 服务表/spawn | 新增 `check_service_console.py` |
| D | — | `apps/{block,fs,appmgr}`；`libs/{virtio,fatfs}`；协议补全 | 新增 `check_service_fs.py`（含磁盘镜像） |
| E | 删 `Runtime` 托管标签、`managed_untyped`、`collect` 缩域 | `libs/user` 移除 Runtime 依赖 | 全量回归 |
| F | `TCBSetPriority`（可选）、IRQ Notification（可选） | init/appmgr 策略完整化 | 新增策略用例 |

## 19. 测试与验收

- 阶段 A（QEMU，新增脚本）：endpoint ping-pong（Call/Reply/ReplyRecv）、badge 识别、cap 传递落点与 Grant 拒绝、消息超长/无 buffer/目标槽非空失败且无半次交付、删除 endpoint 唤醒等待者、fault 投递 + WriteRegisters 修复 + Reply 恢复、无 fault_ep 时终止行为不变、Untyped 切分/递归 Revoke/子区间重启复用、Mint badge AND 语义。
- `init` 起 console；杀死 console（STOP 与直接 fault 两种）；init 重启；console 恢复输出。
- `block` 崩溃 → 重启 + backoff；`fs` 收到 `DEPENDENCY_LOST`。
- 依赖顺序：console 未 READY 前不启动 block。
- STOP 握手：服务正常 flush 后退出；超时强制回收（`forced` 记录）。
- restart storm：超过 `max_restarts` → `Failed`，不再重启。
- `userboot` 在 init 崩溃后重建并重启 init（有限次 + 退避）。
- 权限：服务拿不到别的服务 cap；设备 cap 只给对应驱动；GIC/timer 永不发布（`check_untyped.py` 已覆盖设备策略）。
- `LOG=off` 下 init/服务仍能经 console 协议输出。
- `mysh`（`tools/check_mysh.py`）：在串口上按提示符依次输入 `ls`/`./hello`/`cat APPS.CFG`/`exit`；断言列出 `HELLO.ELF`/`APPS.CFG`、打印文件内容、`[mysh] hello exited: 0` 后 `[mysh] bye`。
- 每阶段同时提供正常用例和权限/失败用例；QEMU harness 区分"预期阻塞 / panic 停机 / 死循环"（沿用现有 check 脚本约定）。

## 20. 与其他微内核对照

| | seL4 sel4test | MINIX 3 | Zircon | 本设计 |
| --- | --- | --- | --- | --- |
| 初始任务 | rootserver fatboot | kernel 加载 boot image | kernel 起 userboot | userboot |
| service manager | 无 | RS | component_manager | init |
| 驱动/FS | fatboot 内 | 独立进程 | 独立进程 | 独立进程 |
| 应用管理器 | rootserver 兼任 | init | component_manager | appmgr |
| 重启 | 父任务手写 | RS | component framework | init（+userboot 监督 init） |
| 配置 | 代码 | 编译期 boot image | 组件清单 | 文本 `init.cfg` |

## 21. 与现有实现的差距

| 现有（代码事实） | 目标 | 阶段 |
| --- | --- | --- |
| `api/dispatch.rs` 对 Send/NBSend/Recv/Reply/ReplyRecv/NBRecv 返回 `Disposition::Fault` | 六个 IPC syscall 完整实现 | A |
| `TcbConfigure` 要求 `fault_ep = 0`（`invoke.rs:236`） | fault_ep 为槽号，故障时在故障线程 CSpace 解析 | A |
| `CNode_Mint` 拒绝 `capData != 0`（`cnode.rs:131`） | Endpoint/Notification badge AND 语义 | A |
| `retype` 白名单无 Endpoint/Notification/Untyped（`invoke.rs:94`） | 三类全部可 retype，Endpoint/Notification 计 64B 记账 | A |
| `finalise_untyped` 单层子对象回收（`cnode.rs:42`） | 深度优先递归 + endpoint 等待者取消 | A |
| `Task` 无阻塞状态/队列链；`Task.execution` 只在创建时写入 | 阻塞状态机 + 受控寄存器交付 | A |
| 单个 `Object::AsidPool`，仅 root 持有 | 监督者持 Copy，逐子进程 Assign | A/B |
| `elf.rs` 依赖 `Runtime::FindEmptySlot` 并向子进程授予 `INIT_RUNTIME` | `SlotAlloc` + 参数化 loader；服务不再拿 Runtime | B（loader）/E（授权） |
| bootloader 归档固定三文件且链接进 loader（`image/archive/mod.rs`），无透传 | archive 透传 RAM + x6/x7 + 共享 `libs/newc` | B |
| `boot_regions` 无 archive 保留区；root CNode 无 archive Frame cap | 记录 id=8 + Frame cap 发布 | B |
| `Object::Runtime` 托管 Create/Start/Wait/Map/WriteMemory + `managed_untyped` + `collect` | 删除/降级，init + appmgr 用户态接管 | E |
| hello 嵌入 fatboot rodata（`build.rs` + `__hello_start`） | archive 中的 `init.elf` + services | B |
| fatboot 兼驱动/FS 计划（`docs/microkernel-design.md` §13） | userboot + 独立服务 | B-D |
| 调度器无优先级（`scheduler.rs`） | 维持；优先级列为阶段 F 候选 | F（可选） |

## 22. 风险与开放问题

| 风险 | 说明 | 缓解 |
| --- | --- | --- |
| 对象表容量 | `MAX_OBJECTS = 4096`：archive ≤ 256 页 + 每服务十余对象 + 应用若干，余量充足但需回归监控；`prepare_boot` 双区间受 1024 页上限 | archive 目标 ≤ 1 MiB；超限在打包期报错；阶段 F 评估 LargePage（seL4 类型 8）摊薄 |
| IPC 路径复杂度 | 阻塞/唤醒/取消交互多，是调度 bug 高发区 | 沿用 park/completion 既有模式；先慢路径；阶段 A 单独成 PR + 专项 QEMU 脚本 |
| 内核/用户 UART 交错 | 内核 log 直写 PL011，与 console 服务无同步 | LOG=off 运行系统；文档化边界（第 16 节） |
| 无 IOMMU 的块驱动 | server 可 DMA 任意物理地址，崩溃隔离不完整 | STOP 握手先停 DMA 再 Revoke；信任边界已写入文档 |
| READY 无超时 | 服务 spawn 后永不 READY 会卡住 init 启动序列（v1 无用户态定时器） | 早期失败走 fault/EXIT 路径；阶段 F 引入启动超时 |
| 子 untyped 预算不可回退 | init 预算中切出的子区间在 init 生命周期内不归还 | 预算校验在 cfg 解析期完成；重启复用同一子 untyped，不新增切分 |
| fault 修复的寄存器写 | `TCB_WriteRegisters` 面向初始启动，故障修复需放宽状态守卫 | 仅对 `TASK_BLOCKED_FAULT` 放宽；PSTATE/PC/SP 校验沿用现状 |

## 23. 已定决策记录

| # | 决策 | 取值 |
| --- | --- | --- |
| 1 | userboot 是否保留 | 保留为 monitor，负责重启 init |
| 2 | 配置形式 | 直接上文本 `init.cfg` |
| 3 | report 通道 | 共享 `control_ep` + badge |
| 4 | 服务预算 | init 切子 untyped，服务崩溃时 Revoke 子 untyped（watermark 重置，预算复用） |
| 5 | 停止语义 | 先实现 STOP 握手，超时强制回收 |
| 6 | 应用启动 | 独立 appmgr |
| 7 | ROM 发布 | 内核发布 archive Frame cap + BootInfo 记录 id=8，userboot/init 自行只读映射（seL4 bootinfo frame caps 风格），不由内核代为映射 |
| 8 | loader 交接 | 扩展为八寄存器：x6/x7 携带 archive 物理范围（本机 ABI，偏离 seL4 六寄存器交接） |
| 9 | fault_ep | seL4 non-MCS 语义：TCB 存槽号，故障时在被监督线程自己的 CSpace 解析 |
| 10 | IPC 首版范围 | 实现 Send/NBSend/Recv/Call/Reply/ReplyRecv/NBRecv；不做 cap unwrap、绑定 Notification、可转移 reply cap、超时 IPC |
| 11 | 设备 Untyped 切分 | 不允许；设备区间整段授予驱动 |
| 12 | 时间源 | 阶段 F 前保留 `Runtime::Clock/Sleep/Exit` 作为用户态时间/退出通道，之后随 IRQ Notification 退役 |
| 13 | Reply 语义 | 隐式一次性 reply 关系（seL4 non-MCS caller 语义的简化），不引入可转移 reply cap |
| 14 | 协议 label 空间 | 0..=4 归 fault；每个用户协议一个 256 宽段（console 0x100/control 0x200/internal 0x300/block 0x400/fs 0x500），Runtime 扩展 0x1000 段；照搬 seL4 按对象类型分连续 invocation label 段的纪律。编译期断言加宿主测试守不重叠 |


## 24. 实施记录（阶段 A–C）

已实施并通过 `make check`（含新增 `tools/check_ipc.py`、`tools/check_userboot.py`）：

- **内核 IPC（阶段 A）**：`Object::Endpoint/Notification`（对象表负载，预算 64B）；六个 IPC syscall（`api/ipc.rs`）实现 seL4 non-MCS 慢路径：queued sender/blocked receiver 状态机、badge 于 x0 交付、`Call/Reply` 隐式一次性 reply 关系（`Caller::Call/Fault`）、`ReplyRecv`、`NBSend/NBRecv`、cap 经 IPC buffer 的 Grant 传递（`transfer_caps` 先全量校验后提交）；`Disposition::Block` 使任务停在已提交的阻塞态，`complete_run` 不再无条件重新入队。
- **fault endpoint（阶段 A）**：`TcbConfigure` 接受 fault 槽号；故障/未知 syscall 组装 seL4 风格消息（VMFault 四 MR 与 ARM 实现一致；UnknownSyscall 重启 PC 跳过 svc）并投递到故障线程自己的 CSpace 解析出的 Endpoint；`Reply` 恢复 restart PC；`TCB_WriteRegisters` 允许监督者修复 `TASK_BLOCKED_FAULT` 线程；`editable()` 对该状态开放 repair 路径。
- **Untyped 切分（阶段 A）**：`Retype(Untyped → Untyped)`（12..=min(30,父) 位、对齐 2^k、记 `ObjectOwner`）；`finalise_untyped` 深度优先递归并取消 endpoint/notification 等待者；`Revoke(子)` 只重置子区间（seL4 `resetUntypedCap` 语义）。`Copy/Mint` 权利衰减扩展到全部类型；Mint badge 遵循 seL4 `updateCapData`：仅未 badged 的 Endpoint/Notification 可烙 badge（=capData），badged 源拒绝派生。
- **BootInfo v6 与 boot module archive（阶段 B）**：bootloader 将整个 archive 按页对齐透传至 RAM 并以 x6/x7 交接（跳转寄存器移至 x16）；内核把 archive 区间纳入保留、以 boot 帧路径发布逐页只读 Frame cap（根 CNode 槽 512 起）并写记录 id=8；ABI 升 v6，`InitialTaskLayout` 预算加入定长记录；共享解析器 `projects/libs/newc`（bootloader 与 userland 同用），允许三个启动镜像之后携带命名模块。
- **userboot / init / console（阶段 B–C）**：`fatboot` 改名 `userboot` 并去掉 hello 嵌入；`elf::spawn_supervised` 支持监督 cap 列表 + 只读 SpawnInfo 参数页 + fault 槽；`init` 解析 `init.cfg`（`rstiny-initcfg`，宿主测试覆盖）、为 console 划子 untyped 预算并监督重启；console 以设备 untyped 重type MMIO 帧并直接驱动 PL011；`rstiny-server` 提供 READY/REPORT/EXIT/STOP 运行时与 `log!`。

实施中确立的语义（与原稿的差异已回写正文）：

- 协议 label 按 seL4 每个对象/协议一个连续段的纪律划分：fault `0..=4`，用户协议各占一段（决策 14）；宿主测试 `projects/libs/protocol/tests/segments.rs` 断言段不重叠。
- `Mint` badge 遵循 seL4：仅未 badged 源可烙；badged 源派生直接报错（决策回写 §6.4）。
- Call 发送方保持 parked 直到 reply（无中间唤醒），避免调度改写阻塞态（§7.3）。
- 归还队列条目惰性剪枝；`Task::destroy` 先 `Runtime::Destroy` 收尾调度任务再 revoke 子树，保证 slot 释放与对象回收（§14.1）。
- 子 untyped 预算需覆盖 CNode 记账（512KB/slot 帧额度）与加载页，init 预算 8MB、console 4MB。
- `Runtime::Clock/Sleep/Exit` 仍在（决策 12 的过渡通道），`Runtime::FindEmptySlot` 已从 loader 路径移除（改由 `LOADER_SLOT_BASE=40000` 起的单调分配，避免与监督者固定槽冲突）。

已知余项：stop 握手的超时强制回收、restart storm 的窗口统计、以及阶段 E–F 全部内容（阶段 D 与依赖拓扑启动顺序、crash→restart 注入已随下述两批实施落地）。

补充实施事实（[独立 fault-handler 线程与线程模型](fault-handler.md) F4 落地时确立）：

- init 的 supervisor/client 线程组已落地：supervisor 是唯一 `control_ep` 接收者，阻塞 `Call` 由 client 线程执行；`crash → supervisor reap → 重启 → console 恢复`的注入用例由 `tools/check_fault_handler.py`（`BOOT_TEST=1`）固化，不再是余项。
- 服务回收链补一条：服务持有的设备区间派生不随其预算子 untyped 回收，必须显式撤销才能重置区间 watermark（§8 的 reset 语义），否则重启的驱动实例切不出设备帧。阶段 D 的实现为**按服务专用副本**：init 持设备母本，spawn 前 copy 出该服务专属副本并授予，teardown 在 `Task::destroy` 前 `Revoke` 副本（见 §25）。

## 25. 实施记录（阶段 D：block/fs/appmgr 与磁盘加载）

阶段 D 已实施（D0–D5，设计见 [disk-driver.md](disk-driver.md) §15 实施记录），本节记录与本文正文相关的修正：

- **spawn 状态机**：spawn 完成置 `Starting`，收到 READY 才 `Running`；依赖扫描只认 Running（正文 §13.2 的"启动顺序"落地）。
- **spawn_service cap 表**：新增 self_ep（子槽 52）、依赖 ep（53..，按 depends 顺序）、每服务设备 Untyped **专用副本**（init 侧 170+i*8+k，spawn 前 copy 母本 161+k，teardown 先 revoke 副本——设备 region 的水位只有 revoke 其派生子树才会复位，预算 revoke 不覆盖驱动 retype 的 MMIO 帧）。
- **loader 并发槽位**：`Supervision.slot_base` = `LOADER_SLOT_BASE + index * LOADER_SLOT_STRIDE`，兄弟服务的 loader 分配窗口互不重叠（正文 §13.4 的 `FindEmptySlot` 移除遗留了这一处共享游标）。
- **预算事实修正**：§24 "console 4MB" 有误——initcfg `budget` 默认 1MB，console 一直用的是默认值。阶段 D 起 userboot 给 init 的预算提升为 24 位（16MB，从独立 region 整块切出、fail-fast），init.cfg 显式声明 console 1M / block 1M / fs 2M / appmgr 4M。
- **内核 IPC 三连修**（阶段 D 调试中确认，均在 `kernel/src/api/ipc.rs`/`task/api.rs`）：
  1. `begin_fault` 现在释放被故障任务的未回复 caller（`fail_caller`），否则监督者停在 `TASK_BLOCKED_REPLY`，故障消息永远无人接收（a44008a"respawn 卡死"的根因之一）；
  2. `reply_phase` 的 reply 投递失败时对已消费的 caller 关系补 `fail_caller`，客户端得到可见错误而非永久 BLOCKED_REPLY；
  3. **等待队列剪枝的角色混淆**：`peek_valid`/`enqueue` 原以"状态 != 本次角色"为陈旧判据，排队的故障发送者会被后续普通发送挤出队列丢弃（监督者永远收不到故障）。现统一为 `stale_entry`（任务不存在或不再等待本端点），角色不匹配的活条目保留。
- **子进程栈** 16 KiB → 64 KiB（debug 构建的驱动/文件栈深度会溢出）。
- KILL_FS 演练由 init 驱动（appmgr READY 后延时回收 fs 并 detach 依赖者），比服务自毁更可控，且不消耗服务的重启预算。
- 阶段 D 的验收脚本：`check_block.py`、`check_fat32.py`、`check_appmgr.py`、`check_services.py`、`check_restart.py`（全部进 `make check`）。
>>>>>>> fafc44a (Implement disk stack D0-D5: VirtIO block, FAT32, apps loaded from disk)
