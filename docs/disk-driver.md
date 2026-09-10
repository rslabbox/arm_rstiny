# 磁盘与 FAT32 用户态驱动设计

日期：2026-09-10。状态：阶段 D（D0–D5）已实施，验收见 `tools/check_block.py`、`check_fat32.py`、`check_appmgr.py`、`check_services.py`、`check_restart.py`；实施记录见文末 §15。

本文定义从启动链到"从磁盘加载用户程序"的完整路径：`block-server`/`fs-server` 的用户态分层、VirtIO MMIO 块设备、FAT32 只读解析、共享内存搬运，以及 appmgr 加载应用。分层参考 Zircon（Fuchsia）的 `userboot → component_manager → 驱动/文件系统 → 应用`，但沿用本项目的对象/capability/Untyped 机制。相关文档：[userboot 与 init 服务管理设计](service-manager.md)、[Untyped 物理内存实现计划](untyped-plan.md)、[内核映射与页表构建](kernel-mapping.md)、[内核实现与验证记录](kernel-implementation.md)。

## 1. 目的与范围

目标：

- 说明 Zircon 与本项目在"应用从哪里加载"上的差别，避免按错误模型设计。
- 给出本项目的启动路径图：现状与目标。
- 规划 `block-server`（VirtIO MMIO 块驱动）与 `fs-server`（FAT32 只读）两个独立用户态服务。
- 定义块/文件协议、共享内存搬运、DMA 与安全边界。
- 规划 `appmgr` 从 FAT32 加载应用，并给出分阶段验收。

范围外：写入、GPT/MBR 分区解析、LFN、网络、SMP、IOMMU/SMMU。

## 2. Zircon 的启动路径（含一处常见误解）

常见误解："Zircon 从 `disk.img` 的 boot 分区加载 `hello`。" **不准确。**

Zircon/Fuchsia 的分工是：

| 分区/来源 | 内容 |
| --- | --- |
| **boot 分区（ZBI / EFI ESP）** | 内核 + **bootfs**（压缩内存文件系统）+ boot items + cmdline |
| **data 分区（FVM → minfs/blobfs）** | 包、blob、应用（`hello` 在这里） |

```text
Firmware (UEFI / coreboot)
  └─ Bootloader
       └─ ZBI  [boot 分区 / -kernel]
            ├─ kernel
            ├─ bootfs (memfs)      ← userboot, component_manager, 初始驱动
            └─ boot items + cmdline
                 │
                 ▼
              Kernel
                 └─ userboot                    (第一个 EL0 进程, from bootfs)
                      └─ component_manager      (from bootfs)
                           ├─ devcoordinator / drivers
                           │    └─ virtio-block  (用户态驱动)   ──┐
                           ├─ filesystem (minfs / blobfs)        │ data 分区
                           └─ pkgfs / package resolver ──────────┘
                                └─ resolve + load hello (blobfs) → EL0 进程
```

结论：**内核加载 `userboot`（bootfs）；`userboot` 加载 `component_manager`（bootfs）；应用由 component framework 从 data 分区（blobfs）解析并加载。** boot 分区不承载应用。例外是 Zircon-standalone 测试镜像会把测试二进制放进 bootfs，但标准 Fuchsia 应用走 data 分区。

## 3. 本项目启动路径：现状与目标

### 3.1 现状（阶段 A-C 已落地）

```text
Bootloader
  └─ Kernel
       └─ userboot                 (root task: map ROM, spawn + supervise init)
            └─ init                (service manager: 目前硬编码 console)
                 └─ console        (UART 设备 Untyped, 轮询 PL011)
```

### 3.2 目标（阶段 D，对齐 Zircon 分层）

```text
Bootloader
  └─ Kernel
       └─ userboot                 ≈ Zircon userboot
            └─ init                ≈ Zircon component_manager
                 ├─ console_server
                 ├─ block-server   (VirtIO MMIO 轮询)   ≈ virtio-block 驱动
                 ├─ fs-server      (FAT32 只读)         ≈ minfs / fatfs
                 └─ appmgr         ≈ pkgfs / loader
                      └─ hello     (从 FAT32 读 ELF, supervised spawn)
```

概念对应：

| Zircon | 本项目 |
| --- | --- |
| bootfs（内存 FS，在 boot 分区） | boot module archive（CPIO/newc） |
| data 分区（blobfs/minfs） | FAT32 磁盘镜像 |
| `userboot` | `userboot` |
| `component_manager` | `init` |
| `virtio-block` 驱动 | `block-server` |
| `minfs`/`fatfs` | `fs-server` |
| `pkgfs` / loader | `appmgr` |

### 3.3 加载流程（目标）

```text
disk.img (FAT32)
   │  QEMU virtio-blk-device (MMIO)
   ▼
block-server  ── Block 协议 ──▶ fs-server ── FS 协议 ──▶ appmgr ──▶ hello
(设备 Untyped)                 (FAT32 解析)            (ELF 装载)
```

## 4. 组件与依赖

| 组件 | 依赖 | 持有 | 职责 |
| --- | --- | --- | --- |
| `block-server` | 无 | VirtIO MMIO 设备 Untyped、子 Untyped | 初始化 virtqueue、扇区读写、容量 |
| `fs-server` | `block-server` | 子 Untyped、Block client cap | FAT32 只读：BPB/FAT/目录/文件 |
| `appmgr` | `fs-server` | 应用 untyped 预算、FS client cap | 读应用 ELF、`spawn_supervised`、应用级重启 |

- 三个组件都由 `init` 按 `init.cfg` 启动与监督（见 [service-manager.md](service-manager.md) §7/§10）。
- `appmgr` 是普通服务，`depends = [fs]`；应用崩溃由 appmgr 处理，appmgr 崩溃由 init 处理。

## 5. 平台与磁盘

### 5.1 平台生成

`tools/build_platform.py` 从 DTB 读取 **status = okay** 的 `virtio_mmio` 节点（compatible `virtio,mmio`，QEMU `virt` 有 32 个槽，每个 `0x200`），生成：

```text
VIRTIO_MMIO_BASE / VIRTIO_MMIO_SIZE   (每个节点)
```

boot 分区把这些 MMIO 区间作为**设备 Untyped** 发布（当前只发布 UART）。不要假设固定 `0x0a000000`：以 DTB 中实际启用、且绑定给 blk 设备的那一个为准。

### 5.2 QEMU

```text
-drive file=disk.img,if=none,format=raw,id=hd0
-device virtio-blk-device,drive=hd0
```

### 5.3 磁盘镜像

先用**裸 FAT32 镜像**（不做 GPT/MBR），由 `tools/make_disk.py` 生成，里面放 `hello.elf`。可用 `mtools`（`mformat`/`mcopy`）或自写 FAT32 writer；产物路径纳入构建产物（如 `target/apps/<MODE>/disk.img`）。

## 6. block-server（VirtIO MMIO，轮询）

### 6.1 初始化序列

1. 校验 `MagicValue = 0x74726976`、`Version = 2`、`DeviceID = 2`（block）、`VendorID = 0x554d4551`。
2. `Status` 握手：`ACKNOWLEDGE(1)` → `DRIVER(2)` → 读 feature → 协商 `VIRTIO_F_VERSION_1` → `FEATURES_OK(8)` → `DRIVER_OK(4)`。
3. 选队列 `QueueSel = 0`，读 `QueueNumMax`，设 `QueueNum`。
4. 在子 Untyped 里 retype 一块**物理连续 Frame** 放 split virtqueue（desc/avail/used），把物理地址写入 `QueueDescLow/High`、`QueueDriverLow/High`、`QueueDeviceLow/High`，最后 `QueueReady = 1`。

### 6.2 Split virtqueue 布局

| 结构 | 对齐 | 说明 |
| --- | --- | --- |
| descriptor table | 16 | `{ addr: u64, len: u32, flags: u16, next: u16 }` |
| available ring | 2 | `{ flags: u16, idx: u16, ring: u16[] }` |
| used ring | 4 | `{ flags: u16, idx: u16, ring: { id: u32, len: u32 }[] }` |

### 6.3 请求

- 一条 block 请求由三段描述符组成：`{ type: u32, reserved: u32, sector: u64 }`（type 0 = IN / 1 = OUT）+ 数据缓冲 + 状态字节。
- 写 `QueueNotify` 后**轮询 used ring**（阶段 D 不用 IRQ；后续接设备 IRQ → Notification）。
- `InterruptStatus` 读后写 `InterruptACK`。
- 容量：config 区 offset `0x100` 读 64-bit `capacity`（sectors，512 B）。

### 6.4 资源与所有权

- MMIO 来自设备 Untyped（只允许 retype 成 Frame，映射 Device/NX）。
- virtqueue 与数据缓冲来自 block-server 的子 Untyped；缓冲**物理地址**交给设备，故必须连续且不被移动。
- 先只支持 512 B 逻辑扇区（QEMU virtio-blk 默认）。

## 7. fs-server（FAT32 只读）

### 7.1 挂载

1. 通过 Block 协议读 sector 0，解析 **BPB**：

   | offset | 字段 |
   | --- | --- |
   | `0x0B` | `bytes_per_sector` (u16) |
   | `0x0D` | `sectors_per_cluster` (u8) |
   | `0x0E` | `reserved_sectors` (u16) |
   | `0x10` | `num_fats` (u8) |
   | `0x24` | `fat_size_32` (u32) |
   | `0x2C` | `root_cluster` (u32) |
   | `0x20` | `total_sectors_32` (u32) |
   | `0x1FE` | `0x55AA` 签名 |

2. 校验 `bytes_per_sector ∈ {512, 4096}`、`sectors_per_cluster` 为 2 的幂、`num_fats ∈ {1,2}`、`fat_size_32 > 0`、`0x52` 处 `"FAT32"`。
3. 计算数据区起点：`data_start = reserved_sectors + num_fats * fat_size_32`；簇 `N` 的首扇区 = `data_start + (N - 2) * sectors_per_cluster`。

### 7.2 FAT 链

- 每个 FAT 项 32 位，有效簇号 28 位（`& 0x0FFF_FFFF`）。
- 特殊值：`0x0FFF_FFF8..=0x0FFF_FFFF` = EOC，`0x0FFF_FFF7` = bad，`0` = free。
- 维护已访问集合或步数上限，检测**循环链**；校验簇号上下界。

### 7.3 目录与文件

- 目录项 32 字节：`name[11]`、`attr` (`0x0B`)、`FstClusHI` (`0x14`)、`FstClusLO` (`0x1A`)、`file_size` (`0x1C`)。
- 阶段 D 只支持 **8.3 短名**；跳过 `0xE5`（删除）、`.`/`..`、`attr & 0x0F == LFN`。
- `attr & 0x10` 为目录；`attr & 0x08` 为卷标。
- 读文件：按簇链逐簇读入共享缓冲，校验 `offset + length <= file_size`、簇边界与扇区对齐。

### 7.4 边界

- 所有解析在**独立进程**内；坏 BPB/FAT/目录/簇链返回错误码，不 panic、不越界。
- 文件大小、FAT 表大小、簇数、目录项数都有上限；FAT32 是经典攻击面，必须可被 init 重启。

## 8. 协议

沿用 [service-manager.md](service-manager.md) 的 label 约定：fault 占 `0..=4`，每个用户协议占一个 256 宽的独立段（block `0x400`、fs `0x500`；control `0x200`、console `0x100`、internal `0x300`）。数据不塞消息寄存器，走共享 Frame；IPC 只传描述。

### 8.1 Block 协议（`block_ep`）

| label | 方向 | 参数 | 返回 |
| --- | --- | --- | --- |
| `BIND` = 0x400 | client → server | 一个可写 Frame cap（共享缓冲） | `max_sectors` |
| `READ` = 0x401 | client → server | `lba`、`sectors` | 状态 |
| `CAPACITY` = 0x402 | client → server | 无 | 扇区总数 |
| `INFO` = 0x403 | client → server | 无 | `sector_size`、`max_sectors` |

### 8.2 FS 协议（`fs_ep`）

| label | 方向 | 参数 | 返回 |
| --- | --- | --- | --- |
| `BIND` = 0x500 | client → server | 共享缓冲 Frame cap | `max_bytes` |
| `OPEN` = 0x501 | client → server | 8.3 短名（打包进 2 个 MR） | `file_id`、`size` |
| `READ` = 0x502 | client → server | `file_id`、`offset`、`length` | 实际读取字节数 |
| `CLOSE` = 0x503 | client → server | `file_id` | 状态 |
| `STAT` = 0x504 | client → server | 短名 | `size`、`is_dir` |

## 9. 共享内存、DMA 与安全边界

- **共享缓冲**：由服务端在子 Untyped 里 retype 一块 Frame，`BIND` 时把 cap 授给客户端；双方映射同一物理页。所有权与最大并发写入协议；一个 client 一个缓冲，避免交叉。
- **DMA**：block-server 把 virtqueue 与数据缓冲的**物理地址**写进 VirtIO 寄存器，设备直接读写这些页。因此这些页在 DMA 期间不能被 revoke/复用；服务重启前必须先停设备并确认 DMA 静止。
- **无 IOMMU**：QEMU virt 没有 SMMU，block-server 是总线主控，属于**受信组件**。文档必须保留该边界，不能宣称其崩溃/恶意一定被隔离。
- **设备 Untyped 只给对应驱动**：VirtIO MMIO 只授予 block-server；GIC/timer 永不发布给用户态。
- **fs-server 面向不可信数据**：解析有界、可重启；不要把它合进 `userboot`/`init`。

## 10. appmgr 与应用加载

- `appmgr` 由 init 按 `init.cfg` 启动，`depends = [fs]`，持有 FS client cap 和应用 untyped 预算。
- 流程：`OPEN("HELLO   ELF")` → 循环 `READ` 到自己的缓冲 → 解析 ELF → `spawn_supervised` 创建应用（独立 VSpace/CSpace/TCB、READY/report/重启协议）。
- 应用与系统服务共用 `libs/server` 协议；控制端点是 appmgr 的 `control_ep`。
- 应用来自 FAT32，因此**替换磁盘上的 ELF 即可改变运行内容，无需重编内核或 userboot/init**——这是与现在"hello 嵌入 rodata"的关键区别。

## 11. 分阶段实施

| 阶段 | 内容 | 前置 |
| --- | --- | --- |
| D0 | 平台生成 VirtIO MMIO + 设备 Untyped；QEMU 加盘 | 现有设备 Untyped |
| D1 | `block-server`：VirtIO 初始化 + 轮询读扇区 | D0 |
| D2 | `fs-server`：FAT32 挂载 + 短名 open/read | D1 |
| D3 | `appmgr`：从 FAT32 读 `hello.elf` 并 supervised spawn | D2 |
| D4 | init 配置驱动化：把 block/fs/appmgr 写进 `init.cfg`、依赖拓扑、按策略重启 | D3 |
| D5 | 服务崩溃重启：杀 fs/block → init 重启 → appmgr 重连；补端到端验收 | D4 |

## 12. 测试与验收

- **D0**：内核发布 MMIO device Untyped；普通任务访问该区间触发故障。
- **D1**：`CAPACITY` 与镜像扇区数一致；读 sector 0 得到与 Rust 侧读取一致的字节；`BIND` 后再读，数据落在共享缓冲。
- **D2**：短名 `OPEN`/`READ` 与镜像内容逐字节一致；坏 BPB、坏 FAT、循环簇链、越界读返回错误且不 panic。
- **D3**：hello 是独立 EL0 进程，正常输出、退出、被回收；替换镜像里的 ELF 后运行内容改变。
- **D4**：`init.cfg` 增加 block/fs/appmgr 后按依赖顺序启动；console 未 READY 前不启动依赖者。
- **D5**：杀 fs-server → init 重启 → appmgr 收到 `DEPENDENCY_LOST`/重连；无内存泄漏（`available` 回到基线）。
- 全部纳入 `tools/check_*`，覆盖 debug/release × LOG=off/info。

## 13. 开放决策

1. 磁盘布局：裸 FAT32（建议）vs GPT/MBR。
2. 共享缓冲：server 持有、client 只读映射（建议）vs client 提供。
3. IRQ：轮询（阶段 D）vs 设备 IRQ → Notification（后续）。
4. appmgr 与 init 的边界：应用清单放 `init.cfg` 还是 appmgr 自己的配置。
5. 文件名：先只支持 8.3 短名，LFN 后置。

## 14. 参考

- 本地 seL4 参考：`../seL4/projects/sel4test/apps/boot/`（VirtIO + FAT32 在 rootserver 内）、`apps/serial/`（独立服务）、`scripts/make_disk.py`（FAT32 镜像）。
- Zircon/Fuchsia：`userboot` → `component_manager` → `virtio-block`/`minfs` → 应用；bootfs 在 boot 分区，应用在 data 分区。
- 项目内：[userboot 与 init 服务管理设计](service-manager.md)、[Untyped 物理内存实现计划](untyped-plan.md)、[seL4 ABI 与内核对象接口](sel4-abi.md)。

## 15. 实施记录（阶段 D0–D5）

已实施并通过验收（`make check` 含全部新脚本，debug/release × LOG=off/info）。与原设计的差异与关键事实：

### D0 平台与设备 Untyped

- `tools/build_platform.py` 新增枚举逻辑：QEMU virt 导出 **32 个** `virtio,mmio` 节点（各 0x200，无 status 属性），整窗 0x0a000000..0x0a004000 共 **16 KiB**，发布为 `VIRTIO_MMIO_BASE/SIZE/SIZE_LOG2`（14 位，按跨度取 2 的幂，非预估的 64K）。单个 0x200 槽不满足 Untyped 最小 4 KiB 对齐，故整窗一个设备 Untyped。
- `kernel/src/boot.rs`：普通 region 仍按 size 降序（`INIT_UNTYPED` 指向最大普通区），**设备 region 独立按 paddr 升序追加**（uart0=12 位 @0x09000000，virtio-mmio-0=14 位 @0x0a000000）——若混入全局降序，virtio 会排到 UART 前，userboot 的首设备授予就会错位。
- QEMU 加盘：`-global virtio-mmio.force-legacy=false -drive ... -device virtio-blk-device`；`tools/make_disk.py` 用 mtools 生成裸 FAT32（无分区表），支持 `--corrupt-bpb`/`--cycle-fat`/`--truncate` 坏镜像注入。

### D1 block-server（第三方库 virtio-drivers）

- 驱动采用 **`virtio-drivers` 0.13**（不移植参考 C 代码）：`MmioTransport::new` 逐 0x200 槽探测 DeviceType::Block；`VirtIOBlk::new` 完成握手/建队；`read_blocks` 经 `Hal::share` 直达 DMA。
- **内核新增 `ArmVspaceTranslate = 47`**：任务在自己的 VSpace cap 上查询任意 VA 的物理地址（限本人 VSpace）。原因是 `Hal::share` 收到的缓冲可能在驱动堆或栈上（BlkReq/BlkResp 是栈上局部量），仅有 DMA 页记录表不够； virtio-drivers 开 alloc 特性时还走 indirect 描述符（表在 BSS 池）。
- `Hal::dma_alloc` 从服务预算 retype 连续 Frame 映射到保留 VA 窗（dealloc 空实现，回收归监督者 revoke）；共享缓冲帧 BIND 时经 `reply_cap` 授予客户端。
- 用户库补 `Page::address()`（ArmPageGetAddress 已有内核实现）。

### D2 fs-server（第三方库 hadris-fat）

- FAT32 解析采用 **`hadris-fat` 2.4.0**（`default-features = false, features = ["read","sync","alloc"]`）。`fatfs` 0.3.6 的 no_std 路径依赖 `core_io`（锁死 2021 nightly）在当前工具链不可编译，且上游久未更新——弃用。
- fs-server 实现 `embedded_io::Read/Seek` 适配器把块协议（BIND 收共享缓冲 cap → READ IPC）伪装成块设备；`FatVolume::open` 挂载，`FatVolumeReadExt::read_file` 读文件。
- 协议约定统一：**回复 label 承载状态码（status::OK=0），MR 承载数据**。

### D3 appmgr 与磁盘加载

- appmgr 读盘上 `APPS.CFG`（复用 `rstiny-initcfg` 解析器）→ `fs::OPEN/READ` 载入 ELF → `spawn_supervised` 逐应用创建进程；应用的 control cap 由 appmgr 的 self_ep **mint**（badged per app），console cap 复制，预算从 appmgr 自己的预算再切子 Untyped（teardown revoke 复位）。
- hello 改用标准服务协议（READY → 经 console 协议输出 → EXIT(0)）；`HELLO_MSG` 编译期变量支撑"换盘换行为"验收。
- 应用 ELF 与清单放磁盘、系统服务 ELF 放 boot archive：替换磁盘文件即改变运行内容。

### D4 init 接入与预算

- `spawn_service` 重写：每服务授予 self_ep（52）、依赖 ep（53..，badge 化）、设备专用副本（init 先 copy 母本再授予，teardown 先 revoke 副本——修复设备 region 水位不复位的泄漏）、控制/console/预算/ASID。
- spawn 状态机细化：`Waiting → Starting（spawn 完成）→ Running（READY）`；依赖仅看 Running。
- **loader 槽位窗口化**：`Supervision` 增加 `slot_base`（`LOADER_SLOT_BASE + index * LOADER_SLOT_STRIDE`），否则并发服务的 spawn 在监督者 CSpace 的 40000+ 槽互踩。
- 预算：userboot 从独立于 root 自用 region 的另一个 ≥24 位 region 整块切出 init 预算（16 MiB，fail-fast），四服务合计 8 MiB（console 1M/block 1M/fs 2M/appmgr 4M）。
- 子进程栈从 16 KiB 提到 **64 KiB**：debug 构建下 virtio/文件栈深度叠加会溢出 16 KiB（现象为 SP 落入镜像与栈之间的未映射空隙）。

### D5 崩溃重启与依赖失效

- 内核修复第三个 IPC 缺陷：**等待队列的角色混淆剪枝**。`peek_valid`/`enqueue` 原以"状态不等于本次扫描/入队的角色"为陈旧判据，导致排队的故障发送者被后续普通发送**挤出队列丢弃**（表现：监督者永远收不到故障，依赖链死亡）。现统一为 `stale_entry`（任务已不存在或不再等待本端点才算陈旧），角色不匹配的活条目保留。
- init 新增 `detach_dependents`：依赖服务死亡时 nbsend `DEPENDENCY_LOST` 通知依赖者（best effort）并强制回收重启依赖者（重启计数共享窗口/退避），重建后依赖链从头 BIND。
- fs/appmgr 处理 `DEPENDENCY_LOST`：清理后退出，由 init 按策略重建；appmgr 退出前先逐个销毁应用，避免应用成为孤儿。
- 验收：`tools/check_restart.py`（KILL_FS=1）断言 hello 前后运行两次、`[appmgr] frames=N` 前后一致（无泄漏）；`check_services.py` 断言依赖序启动与 block 失败时 fs/appmgr 不启动。

### 与设计文档的偏差汇总

| 项 | 原设计 | 实施 |
| --- | --- | --- |
| VirtIO 驱动 | 自研（参考 seL4 C 移植） | `virtio-drivers` 0.13 crate + Hal 适配 |
| FAT32 | 自研解析器 | `hadris-fat` 2.4.0 只读挂载 |
| 设备 Untyped | 64K 窗口（预估） | 实测 16K（32×0x200），size_bits=14 |
| 内核新增调用 | 无 | `ArmVspaceTranslate`（DMA VA→PA 自翻译） |
| SpawnInfo | extra[0..3] | extra[0..7] 全占用（self ep/依赖数/依赖 ep 槽） |
