# ARM RSTiny：seL4 风格微内核设计与实施路线

日期：2026-09-08。状态：构建基线、内核底座和最小 EL0 fatboot 已实施，单核内存管理和轮转调度也已实施；完整能力系统及 IPC 等仍为设计提案。参见 [内核实现与验证记录](kernel-implementation.md) 和 [fatboot 启动文档](fatboot.md)。

本文中的“当前项目”和 `src/` 路径记录的是最初设计时的基线；后续结构已迁移为 `kernel/src/`，现状及运行命令以实现记录和根目录 README 为准。

本文的初始分析基于当时项目源码，以及 `../seL4` 中实际存在的内核和用户程序。初次分析时项目 HEAD 为 `2c8563c`，参考内核 HEAD 为 `28b8f4c40`；参考目录存在本地修改，本文描述的是查阅时的工作区内容，不能仅凭提交号复现全部参考行为。

当前实现进度：内核底座与最小 EL0 fatboot 已完成，现已补齐 [单核用户内存与任务调度](memory-task.md) 和 [Rust bootloader 引导链](boot.md)。当前父子任务句柄不是完整 CSpace；原路线中的能力系统仍待实现。实际 ABI、高地址内核映射和验证见 [fatboot 启动文档](fatboot.md)。下文旧代码分析保留为设计背景，后续章节为路线规划。

## 1. 先回答阶段划分

你的理解基本正确，但需要明确三个边界。

1. **第一步：建立只含必要机制的内核底座，保留可关闭的 debug 串口。** 此时可以没有用户程序，但还不能称为完整微内核。页表、异常、上下文切换、中断控制器和调度定时器属于内核的必要硬件支持，不能把“没有驱动”理解为没有任何硬件相关代码。磁盘、网卡、文件系统和普通串口服务放在用户态。
2. **第二步：启动第一个真正运行于 EL0 的 root task。** 参考项目里的名字实际是 `fatboot`，不是 `fastboot`。此时先做最小 fatboot：接收 BootInfo、执行用户代码、通过 SVC 进入内核并返回；不要求马上读取 FAT32。
3. **后续再把 fatboot 做完整。** 它要创建串口服务、读取磁盘、加载 ELF、创建应用，必须先具备能力管理、地址空间管理、线程调度和 IPC。

内核日志统一由构建时的 `LOG` 控制，默认 `info`，支持 `off/error/warn/info/debug/trace`。`log` 为普通依赖，优先使用标准日志宏；`LOG=off` 时普通日志静默；panic 无条件绕过 LOG 过滤直接打印，UART 映射始终保留。日志等级与 Cargo dev/release 优化配置独立。

## 2. 复刻范围与首个最终目标

第一版目标是用 Rust 实现 seL4 风格的机制和隔离边界，以及本地参考系统的启动闭环：

```text
QEMU / 启动加载器
  → Rust 微内核初始化
  → EL0 fatboot（root task）
  → EL0 serial_server（独立地址空间）
  → fatboot 的用户态 VirtIO 块设备与 FAT32 库
  → 从磁盘读取 HELLO.ELF
  → EL0 hello（独立地址空间）
  → hello 通过 IPC 请求 serial_server 输出
```

第一版采用以下约束：

| 项目 | 选择 |
| --- | --- |
| 平台 | QEMU AArch64 `virt`，Cortex-A72，单核 |
| 特权级 | 内核 EL1，用户程序 EL0；QEMU 固定 virtualization=off，以 EL1 启动 |
| 内存 | 4 KiB 页；最初固定受支持的 RAM 范围，之后读取 DTB |
| 调度 | 简单固定优先级抢占，同优先级轮转，非 MCS |
| 隔离 | 每个进程独立 VSpace 与 CSpace；内核检查所有对象操作权限 |
| 用户程序 | 静态链接 AArch64 ELF64，第一版只支持 ET_EXEC |
| 文件系统 | 先只读 FAT32 |
| 设备 | PL011 输出、VirtIO MMIO 块设备；网络后置 |
| 对外接口 | 自有、有版本的 Rust/C 可表达 ABI，借鉴 seL4 概念 |

本路线不承诺直接运行现有 seL4 二进制，也不继承 seL4 的形式化证明。现有 C 程序依赖 `libsel4`、`sel4utils`、`allocman` 等接口，不能只替换内核就继续运行。先移植应用逻辑到本项目用户库；若以后要求兼容，再单独建立 syscall 编码、对象布局、BootInfo、错误码和调度语义的兼容矩阵。

SMP、MCS、虚拟化、动态链接、POSIX、网络栈、磁盘写入与形式化验证不进入第一版关键路径。FP/SIMD 第一版应禁用并捕获相关用户异常，或完整保存恢复状态后才放开；不能因使用 softfloat 编译目标就假设任意外部 ELF 都不会使用这些寄存器。

## 3. 当前项目与参考系统的差距

### 3.1 当前项目已有内容

| 位置 | 当前行为 | 后续处理 |
| --- | --- | --- |
| `src/arch/boot.rs` | 启动栈、EL 切换、启动页表、开启 MMU | 保留思路，审计并拆分启动映射和运行期映射 |
| `src/arch/page_table.rs` | 页表项与权限位构造 | 扩展完整页表遍历、映射、撤销、TLB 操作 |
| `src/arch/trap.S`、`context.rs` | 异常入口与通用寄存器保存恢复 | 用作用户态入口、syscall 和线程切换基础 |
| `src/arch/trap.rs` | 同步异常及 IRQ 主要打印日志 | 按异常来源与 ESR 分类处理，增加调度和故障路径 |
| `src/utils/console.rs` | 内核直接访问 PL011 | 缩为编译期可选 debug 输出后端 |
| `src/utils/logging.rs` | `LOG` 选择日志级别，普通打印绕过级别 | 普通输出统一走 LOG，panic 使用独立应急输出 |
| `src/utils/heap_allocator.rs` | 固定 16 MiB 内核堆，初始化时直接打印 | 早期可保留；后续对象内存改由 Untyped 授权管理 |
| `src/user.rs` | 在 EL1 调用分配器和文件系统测试 | 更名为内核自测入口；真正用户程序另建 ELF |
| `src/drivers/`、`src/test/fatfs_perf.rs` | 内核内 VirtIO/FAT 测试路径 | 迁至用户态，内核默认构建去除这些依赖 |
| `Makefile` | 默认挂块设备、网卡；镜像名称硬编码 | 增加按阶段运行目标，修正构建产物路径 |

`rust_main()` 直接调用 `user::user_main()`，没有构造 EL0 初始上下文并执行用户入口，因此目前没有用户态进程。文件名包含 user 并不改变执行权限。

需要优先检查的具体事项：

- 当前启动页表让 TTBR0 与 TTBR1 指向同一根，并对 RAM 使用大块 RWX 映射。这只能作为早期过渡，不能直接作为进程隔离方案。
- QEMU 声明 4 GiB RAM，启动代码当前只映射其中从 `0x4000_0000` 起的 1 GiB RAM。分配器必须仅管理已支持且真实存在的区间，不能直接按 4 GiB 发放 Frame。
- 页表数组必须明确保证 4 KiB 对齐；当前类型本身未表达该约束。启动栈顶部的形成、`adrp` 的对齐假设、BSS 清零范围也应通过链接映射验证。
- 当前同 EL 与低 EL 同步异常走同一个处理函数，且处理后直接返回。未修复的数据/指令异常可能不断重新触发；必须区分内核致命故障和用户故障。
- 当前 `current_ticks()` 只读计数器，不代表已经实现定时器中断和抢占。
- 最初构建脚本存在包名与产物名称不一致的问题；现在 package 与 binary 统一为 `kernel`，构建脚本按该名称生成镜像。

这些是源码审查结果和实施前检查项；本文没有运行当前内核来验证其运行时行为。

### 3.2 本地 seL4 示例已经如何组织

`../seL4/projects/sel4test/apps/CMakeLists.txt` 声明 `DeclareRootserver(fatboot)`。

`apps/boot/main.c` 接收 BootInfo，建立用户态分配和地址空间管理，再启动串口服务、访问块设备、挂载 FAT32、读取 `HELLO.ELF`、创建子进程并处理其故障/完成消息。

`serial_server` 是独立 ELF，嵌入 fatboot 镜像中，从而不依赖磁盘。参考实现为它映射 UART 页，通过 endpoint 提供轮询式串口输出。`hello` 则单独写入 FAT32 镜像，不嵌入 rootserver。

因此，参考系统目前是“fatboot 内含块设备/FAT32 用户态库 + 独立串口服务”，并非每个驱动都已经独立成进程。先复现这一粒度，再考虑拆成 `block_server` 和 `fs_server`。

## 4. 内核与用户态职责

| 能力 | 内核负责 | 用户态负责 |
| --- | --- | --- |
| CPU | 异常、系统调用、上下文切换、抢占 | 程序执行、业务逻辑 |
| 内存 | 验证对象创建、建立映射、隔离、TLB 维护 | 决定内存分配给谁、用户堆、装载程序 |
| 权限 | CSpace 查找、对象类型和权限检查、能力撤销 | 初始权限拆分与分发策略 |
| 通信 | Endpoint、Notification、消息交付与阻塞 | 控制台、文件、网络等协议 |
| 中断 | GIC 配置、确认/屏蔽/结束中断、授权投递 | 操作设备、清除设备中断源、请求重新启用 |
| 计时 | 调度所需架构定时器 | 更高层计时服务及策略 |
| 串口 | 可选 debug 轮询输出 | 正常串口驱动和日志服务 |
| 存储 | 授权设备页和必要 DMA 资源 | VirtIO 协议、FAT32、文件读取 |
| 程序启动 | 启动第一个用户任务、配置 TCB 的机制 | 后续 ELF 装载、进程创建策略、服务监管 |

内核不需要通用 `open/read/write/fork/exec`；第一版用对象调用和 IPC 组合这些服务。微内核的关键是权限与隔离边界，不能仅通过删除几个驱动文件来达成。

## 5. 功能依赖与实施顺序

| 阶段 | 可见成果 | 依赖 | 此时不要求 |
| --- | --- | --- | --- |
| 构建基线 | 构建、无盘启动、源码边界可复现 | 当前代码 | 新内核功能 |
| 内核底座 | 仅内核启动，debug 可完全关闭，异常可定位 | 构建基线 | 用户程序、文件系统 |
| 首个用户任务 | 最小 fatboot 在 EL0 运行，SVC 往返，访问隔离 | 内核底座 | 从磁盘装载程序 |
| 能力与对象 | fatboot 用 Untyped/CSpace 创建对象及地址空间 | 首个用户任务 | 通用 IPC 服务 |
| 多线程与抢占 | 多个用户线程/地址空间可独立运行 | 能力与对象 | 设备服务 |
| IPC 与故障 | Call/Recv/Reply、通知、故障监管 | 多线程与抢占 | 磁盘 |
| 用户态串口 | serial_server 输出，内核设置 LOG=off 仍可用 | IPC 与故障 | UART 接收中断 |
| 完整 fatboot | 用户态读 FAT32，装载磁盘 ELF 并运行 hello | 用户态串口 | 驱动逐个独立进程 |
| 设备中断与服务拆分 | IRQ 通知，块设备/文件系统可独立服务 | 完整 fatboot | SMP、网络 |
| 完善与优化 | 生命周期、稳定性、基准、可选后续能力 | 设备中断与服务拆分 | seL4 证明/二进制兼容承诺 |

首个用户任务与能力系统应逐步实现，不等于这时已经具备完整 seL4 语义；所有尚未支持的操作应明确返回错误，不能静默绕过权限检查。

## 6. 固定可复现构建基线

目标：后续每阶段有可比较的构建和运行入口。

实施内容：

1. 保存当前分配器和 FAT 性能测试的调用方式，移出默认内核启动路径。
2. 固定 AArch64、单核、QEMU `virt`；显式选择初期 GIC 版本，例如 GICv3，并确保对应实现一致。
3. 修正 ELF/bin 路径；分别管理内核、用户 ELF、启动镜像和磁盘镜像。
4. 增加“无磁盘、无网卡”的最小启动目标；内核初始化不触发设备扫描。
5. 将平台常量、内存属性类型和分配器解耦，避免页表模块依赖堆分配器定义 `MemFlags`。

已提供 `make run-kernel` 与 `make check`；`make run-root`、`make run-system` 留待对应功能实现。

验收：从干净构建产物启动成功，构建不读取旧 ELF；无磁盘时不进入 FAT 测试。记录 QEMU 命令、工具链版本、入口 EL 与 RAM 范围。

## 7. 内核底座与可选 debug 串口

### 7.1 最小启动流程

当前已采用 Rust bootloader：固定从 EL1 启动，临时映射和启用 MMU 由 loader 完成，内核以 MMU/cache 已开启的 EL1 状态接收六个启动参数。运行期页表和 root task 初始化由 Rust 内核负责，详见 [引导链](boot.md)。

```text
_start
  → 判断入口 EL、仅启动主核
  → 建立启动栈并清零需要清零的 BSS
  → 建立临时映射、设置 MAIR/TCR、启用 MMU
  → 安装异常向量和内核运行期映射
  → 建立受控物理内存区间与启动分配器
  → 初始化串口，供日志和 panic 输出使用
  → 进入内核 idle/测试入口
```

当前仅支持实际验证过的 QEMU EL1 入口，不实现其他 EL 的转换路径。GIC/定时器可以在 内核底座 建立骨架，在 多线程与抢占 完成抢占闭环，不能让空 IRQ handler 配合未清中断源的硬件运行。

### 7.2 必须形成的约束

- 内核 text 为 RX，rodata 为 R/NX，data/栈/堆为 RW/NX；UART 页为 Device/NX。
- 临时大页映射须在进入用户态前收缩，消除残留的可写代码别名和无必要的设备映射。
- 不受支持的内核异常记录故障状态并停机，不返回到原故障指令。
- panic 路径不依赖堆，不因串口锁重入死锁；输出应有轮询上限。
- 内核空闲使用适当等待指令；尚未启用中断时的停机状态不伪装为可被调度唤醒的 idle。

### 7.3 日志配置

使用构建环境变量 `LOG`，不再增加独立的内核 debug/printing feature。构建脚本校验等级，未设置时默认 info，并在 LOG 改变时触发重新构建。

| 配置 | 内核串口日志 | 后续用户串口服务 |
| --- | --- | --- |
| dev/release + LOG=info | info 及更严重的日志 | 可独立运行 |
| dev/release + LOG=error | 普通日志仅 error；panic 独立打印 | 可独立运行 |
| dev/release + LOG=off | 普通日志无；panic 仍打印 | 可独立运行 |

启动、heap 初始化、自测和异常日志使用标准 `log` 宏，panic 使用独立串口应急输出，不提供绕过过滤的普通打印旁路。LOG=off 时标准日志宏不求值消息参数；不要求日志实现从二进制消失。内核逻辑不能依赖日志表达式中的副作用。

所有配置发生致命故障后均记录状态、屏蔽中断并停机；LOG=off 时 panic 仍打印，其他状态可由调试器读取。`kernel-test` 独立控制自测与故障探针，不改变日志配置。未来 DebugPutChar 若实现，也必须服从日志关闭策略。

验收：dev/release × LOG=off/info 均能启动；关闭日志时正常启动无输出，panic 仍有诊断；其他等级正确过滤。注入异常核对状态和诊断，不能仅凭静默认定启动成功。

## 8. 首个用户任务：首个 EL0 程序——最小 fatboot

### 8.1 首个程序从哪里来

第一版构建 `projects/apps/fatboot` 为独立 ELF；宿主打包工具验证其装载段，生成带入口、段描述和字节内容的 boot module，与内核打包。内核只映射/拷贝描述中的段并建立首个任务，不读取 FAT32，也不提供通用磁盘 ELF loader。

后续可以换成独立 loader 同时加载内核和 root task。无论采用哪种包装方式，fatboot 都必须拥有独立用户地址空间与 EL0 执行权限；嵌入镜像并不等于在内核态执行。

### 8.2 内核准备的资源

- 一个初始 TCB、内核异常栈和初始 CSpace。
- 独立 TTBR0 用户页表；TTBR1 放置仅 EL1 可访问的高地址内核映射。当前已实现，并验证用户与内核访问权限。
- 用户 text RX、data/BSS RW/NX、用户栈 RW/NX，栈旁留 guard page。
- 用户只读 BootInfo 和专用 IPC buffer；物理页不与内核对象重叠。
- 初始寄存器：ELR_EL1 = 入口、SP_EL0 = 0（root 运行库入口设置为 ELF 提供的对齐栈顶）、SPSR_EL1 = 合法 EL0t 状态、x0 = BootInfo 用户虚拟地址；其他寄存器清零。

用异常返回路径执行 `eret`；后续异常必须落到内核受控栈。完成新装载代码的 D-cache/I-cache 一致性维护，并按架构要求处理页表写入和 TLB 屏障。

### 8.3 BootInfo 与初始系统调用

BootInfo 是自有 ABI，不直接照搬 seL4 的内存布局。至少包含：magic、ABI version、结构长度、页大小、支持的功能位、IPC buffer 地址、初始 capability 槽号、可用槽区间、启动模块信息。能力与对象 添加 Untyped 描述数组；以后通过带类型和长度的扩展记录传递 DTB。

首个用户任务 可以只发布静态初始对象和有限操作。当前初始任务上下文与 VSpace 是启动特例，尚无 CSpace，不能创建任意对象，不能宣称已有完整 rootserver 能力。

建议第一批 syscall：`Yield`、`ThreadSuspendSelf`、仅 debug 可用的 `DebugPutChar`。暂停自己后内核运行 idle；用户代码不能通过普通返回地址返回 `rust_main()`。debug 系统调用使用固定字节参数，避免为最早调试接口引入任意用户指针读取。

验收：

1. GDB 或异常记录确认用户 PSTATE 为 EL0t，SVC 来源为 Lower AArch64；不要求 EL0 读取可能受限的系统寄存器来证明身份。
2. SVC 往返后保存的用户寄存器、SP 和 PC 正确。
3. 用户访问内核页、未映射页、直接访问 UART 均触发用户故障；用户写 text 或执行栈也失败。
4. 在 IPC 与故障 故障 IPC 实现前，非法访问使该线程进入 Faulted 并交给 idle；不重新执行故障指令，也不让它破坏内核。
5. 设置 LOG=off 后同样进入用户入口；通过 GDB 断点或测试结果页验证。

## 9. 能力与对象：Capability、Untyped 与地址空间管理

这是从“能运行用户程序”进入“seL4 风格微内核”的关键阶段。

### 9.1 对象与权限

| 对象 | 作用 | 初期操作 |
| --- | --- | --- |
| Untyped | 某段物理内存的创建权限 | Retype |
| CNode | 内核维护的 capability 槽表 | Copy、Mint、Move、Delete、Revoke |
| TCB | 线程寄存器、调度状态、关联资源 | Configure、Read/WriteRegisters、Resume、Suspend |
| Frame | 普通物理页或设备页 | Map、Unmap |
| PageTable/VSpace | 用户地址空间及页表结构 | 安装页表、建立/删除映射 |
| Endpoint | 同步 IPC 队列 | IPC 与故障 实现 Send/Recv/Call |
| Notification | 异步位集合通知 | IPC 与故障 实现 Signal/Wait |
| IRQControl/IRQHandler | 中断线路的控制权 | 设备中断与服务拆分 实现授权、绑定和 Ack |

用户传入 `CapPtr` 槽号，内核在调用者 CSpace 中查找对象、验证类型和权限。CapPtr 不是地址，修改一个整数不会创建权限。第一版允许单层 CNode；多级 guard/radix 查找后置，并标明与 seL4 的差异。

内核用稳定 ObjectId 和 generation 或等价机制识别对象。能力记录含对象引用、权限和适用对象的 badge；派生关系用于撤销。badge 由内核从 endpoint capability 交付，不能信任消息正文里自报的客户端身份。

### 9.2 内存创建流程

启动时先排除内核镜像、栈、页表、启动模块、BootInfo、初始对象等保留区域，再把剩余受支持 RAM 切成合法对齐的 Untyped 区间交给 root task。设备资源单独列为 device Untyped/Frame；保留的 GIC、内核 timer 相关资源不交给普通驱动。

`Retype` 必须验证类型、对象尺寸、对齐、数量溢出、剩余空间、目标槽为空及持有权限。失败前不产生半初始化对象；若采用分步提交，必须能回滚。普通对象内存对新所有者清零；设备内存不清零且只允许生成设备 Frame。

用户态选择如何分配 Untyped，内核执行并验证对象构造。运行期不得由一个无限增长的全局内核堆偷偷为用户创建对象；内核元数据也应来自有界、明确计费的预留空间或对象资源。

### 9.3 删除与回收

能力与对象 初期可以使用只增分配器，但必须明确限制：暂不支持安全回收时，相关操作返回 Unsupported，不能把仍被引用的内存重新发放。能力与对象 完成需至少支持 Frame、页表和静态线程资源的正常解除引用；Endpoint 等生命周期随对应阶段补齐。

完整回收顺序：停止使用者 → 移出运行/等待队列 → 删除映射并完成 TLB 失效 → 清理绑定与能力派生引用 → 在确定无可达引用后复用内存。`Delete` 删除一个 capability，不意味着同一对象其他 capability 自动失效；`Revoke` 撤销派生权限，不能只把某个槽清空。

最初可以规定一个 Frame capability 只记录一处映射，另一次映射需要派生 capability，避免遗漏反向映射信息。禁止映射任意物理地址；所有映射必须由有效 Frame 和 VSpace 权限共同授权。

验收：伪造槽号、错误对象类型、只读权写映射、覆盖已有槽、重复使用物理内存均失败；新页清零；合法 Retype/Map/Unmap 成功；撤销后旧权限不可用，复用后旧句柄不能访问新对象。

## 10. 多线程与抢占：多线程、地址空间切换与抢占

TCB 至少保存用户上下文、CSpace/VSpace 引用、优先级、时间片、故障处理端点和队列节点。状态建议为 Runnable、Running、BlockedSend、BlockedRecv、BlockedReply、BlockedNotification、Suspended、Faulted；尚未实现的 IPC 状态在 IPC 与故障 接入。

调度器维护每优先级 FIFO 就绪队列，选择最高优先级可运行线程；同优先级按时间片轮转，队列为空运行 idle。单核内核临界段先禁止抢占，设置单独的重调度标记，在退出内核时决定切换。

root task 可在自己的授权优先级范围内配置子线程；不能让普通线程随意把优先级提升到系统服务之上。第一版明确存在严格优先级饥饿与 IPC 优先级反转限制，不宣称具备实时保证。

完成 GIC 与架构定时器的实际中断链路：装载下一次截止值、解除屏蔽、识别中断、正确确认/结束、更新时间片。不得把“IRQ 打印一次”当成调度器完成。

每次切换加载用户页表与上下文。最初可用保守的适当范围 TLB 失效保证正确性，再加入 ASID；复用 ASID 必须失效旧翻译。内核栈/TrapFrame 所属关系应唯一，不允许两个可挂起线程覆盖同一保存区。

验收：两个独立 VSpace 在相同 VA 映射不同内容；忙循环线程不主动 Yield，另一同优先级线程仍持续进展；挂起线程不再执行；全部暂停后进入 idle，定时器仍能唤醒内核。

## 11. IPC 与故障：IPC、Notification 与用户故障

### 11.1 同步 IPC

先实现普通慢路径：`Send`、`Recv`、`Call`、`Reply`、`ReplyRecv`。Endpoint 维护发送/接收等待队列，没有对端时阻塞；`Call` 在请求被接收后等待回复，不能在请求送达时提前变成可运行。

第一版建议寄存器传递最多 4 个机器字，随后用专用 IPC buffer 支持有上限的大消息。所有长度、cap 数量和用户地址都要检查；按明确顺序完成校验与消息提交，错误时不留下半次通信。

回复权限必须与一次具体 Call 和调用者关联，只能消费一次；不能依靠用户传入“要唤醒的 TCB id”来恢复线程。初版可将回复 token 存在接收线程的受控槽位，禁止尚未回复时覆盖，后续再支持显式保存和嵌套调用。

能力传递可分两步实现：先由 root task 预先填好双方 CSpace，再增加受 Grant 权限约束的传递。消息协议必须声明当前是否支持 extra caps，不能忽略未知字段。

### 11.2 Notification 与故障

Notification 记录 pending 位集合，多次 Signal 可合并；它不是记录每次设备事件的消息队列。驱动收到通知后必须读取设备状态/完成队列直到处理完毕。

用户页故障、非法指令和不支持的执行状态应产生带类型的故障消息，包含故障地址、PC、访问类型等必要字段；内核向已配置的监管端点交付，故障线程保持阻塞。未配置处理者时停止该线程。未知 syscall 可返回 Unsupported，与不可恢复用户异常区分。

修复后恢复线程必须通过受控回复/TCB 操作，并校验 PC、SP、PSTATE；无法修复则终止。内核自身异常走独立 panic 路径。接收故障、处理普通输出、处理应用完成状态应有明确协议边界。

验收：双向 IPC 和重复 Call/Reply 正常；非法消息、无权限端点、重复回复均失败；接收方退出或对象撤销时清理等待者，不永久悬挂；子程序越界访问只使该子程序停止，监管者和另一个程序继续运行。

## 12. 用户态串口：独立用户态串口服务

fatboot 从随启动模块提供的 `serial_server.elf` 创建服务，为其配置独立 CSpace/VSpace、UART Device Frame 和接收 endpoint。UART MMIO 页只映射给串口服务；客户端只持有具备发送/调用权限的 endpoint capability。

参考本地示例，先做轮询 TX，不依赖 UART IRQ。服务显式初始化需要的寄存器状态或记录固件前提，并对轮询设置有界失败行为，避免设备异常让服务永久卡住。

控制台协议定义版本、操作号 `ConsoleWrite`、长度上限、返回状态；客户端按上限分块。消息边界内的输出由服务串行处理，badge 用于识别授权客户端；过长或不支持的请求返回错误。

内核 debug 与用户 UART 共享设备时，不能依赖跨地址空间自旋锁保护。第一版采用“启动期内核 debug → 显式交接 → 用户串口服务”策略：交接后普通内核日志不再写该 UART；致命 panic 可在停止正常执行后应急接管输出。引入多核或并发设备访问前，需实现停核协调或提供独立 panic 串口。

验收：无磁盘可以启动串口服务；设置 LOG=off 后 fatboot 和测试客户端仍可经 IPC 输出；普通客户端没有 UART 映射且直接访问会故障；非法请求不影响下一条正常请求。用户服务启动失败由监管者记录状态，不能只依靠尚未启动的串口报告。

## 13. 完整 fatboot：完整 fatboot——磁盘、FAT32、ELF、hello

### 13.1 用户态 VirtIO 块设备

将 `src/drivers/hal.rs`、存储相关逻辑适配为用户库。MMIO 通过授权 Frame 映射；DMA buffer 来自 fatboot 授权持有的 RAM；虚拟地址不能强转为设备地址。初期固定 QEMU 无 IOMMU 平台，明确 DMA 地址等于授权物理地址的适用条件。

显式固定和记录支持的 VirtIO transport/version/features，不依赖未知默认行为。先只读、单请求、轮询完成：检查容量、扇区范围、描述符链、完成状态、超时和内存屏障。DMA cache 一致性和必要维护操作必须在 HAL 中有明确实现。

CPU 页表不能限制设备 DMA。没有 IOMMU/SMMU 时，持有总线主控设备的驱动属于受信任组件，不能声称其崩溃或恶意行为一定被地址空间隔离。这是本阶段隔离结论的明确边界。

### 13.2 FAT32 与 ELF 装载

FAT32 第一版只读，挂载支持明确的分区布局。按镜像工具约定查找 `HELLO.ELF`；如果先只支持短文件名，要在工具和库两侧一致限制。检查扇区大小、分区/簇范围、FAT 链循环、文件大小上限以及算术溢出。

fatboot 的 ELF loader 支持 ELF64、小端、AArch64、ET_EXEC：

1. 校验 header、program header 表的文件边界和长度计算。
2. 校验 `PT_LOAD` 的 filesz ≤ memsz、文件区间、VA 区间、对齐和页内偏移关系。
3. 拒绝落入内核地址、用户保留页、入口不在可执行段，以及不支持的解释器/动态装载需求。
4. 第一版让用户链接脚本生成互不重叠的装载页；遇到段共享页则明确拒绝，后续再实现正确权限合并，禁止意外产生 RWX 页。
5. 创建目标 Frame，通过受控装载窗口复制并清零 BSS，关闭可写装载别名，再建立最终 RX/RW 权限并同步指令缓存。
6. 建立栈、用户启动参数页、IPC buffer、CSpace 和 TCB；只有全部成功后 Resume，失败时回收未发布资源。

用户 ELF 的入口约定由本项目 CRT 实现，不默认等于 Linux 或 seL4 musl 的入口约定。首版向 x0 传子进程启动信息指针，包含版本、argc/argv（如支持）、控制台 cap 和监管 cap；用户库把它转换成应用调用。

### 13.3 权限分发与验收

hello 只获得自己的执行资源、控制台客户端 capability 和完成通知能力。它不获得全局 Untyped、其他 TCB、UART MMIO 或 VirtIO MMIO。

验收闭环：

```text
内核 → fatboot → serial_server
fatboot → VirtIO 读盘 → FAT32 → HELLO.ELF
fatboot → 新 CSpace/VSpace/TCB → hello
hello → ConsoleWrite IPC → serial_server → UART
hello → 完成协议 → fatboot 监管与回收
```

仅替换磁盘上的 hello 即可改变运行内容，无需重新链接内核或 fatboot。拔掉磁盘、文件缺失、损坏 FAT、截断 ELF、非法段权限分别返回可诊断错误；串口服务继续工作。此阶段跑通后，才算复现本地参考系统的主要可见功能。

## 14. 设备中断与服务拆分：设备 IRQ 与进一步拆服务

多线程与抢占 已实现调度 timer IRQ；此阶段实现的是用户设备 IRQ 授权与交付。

root task 通过 IRQControl 为目标线路创建 IRQHandler，绑定到驱动 Notification。硬件中断发生后，内核按 GIC/触发类型协议确认并屏蔽或保持受控状态，发送通知；驱动读写设备清除源，再调用 Ack 请求重新开放。EOI、deactivate 和屏蔽的顺序由实际 GIC 版本明确规定。

保留定时器及内核专用 IRQ，拒绝重复或无权绑定，限制驱动可控制的线路。针对电平中断测试“设备源未清除”的重复触发，不能无限高速进入内核。

然后逐步拆分：`fatboot → block_server → fs_server → 应用`。块数据和文件数据经授权共享 Frame 搬运，IPC 只传请求描述、长度、偏移和完成状态；定义缓冲区所有权、最大并发、超时和服务重启后的失效规则。

验收：块设备由轮询改成通知后读盘正确；重复/合并通知不会丢失完成项；驱动没有 GIC MMIO 权限；服务故障可使等待客户端收到错误或由监管者处置。具备 DMA 的服务重启前须停止设备并确认 DMA 静止，之后才能撤销/复用缓冲区。

## 15. 完善与优化：完善、回归与性能

优先补齐对象撤销、线程死亡、IPC 等待者唤醒、进程资源回收和服务重启；再测量 IPC 延迟、上下文切换、映射开销与镜像尺寸。先保证慢路径正确，再引入 fastpath、ASID 优化、批量日志和共享内存性能改进。

SMP 需要独立设计跨核调度、锁、IPI、TLB shootdown 和跨核回收；不能仅把 QEMU `-smp` 改大。MCS、SMMU、FP/SIMD、网络和兼容层分别作为新里程碑，不插入前七步的启动闭环。

## 16. 建议代码组织

按阶段迁移，避免第一步只做大范围搬目录而无法运行：

```text
src/
  main.rs
  arch/aarch64/       # 启动、trap、上下文、页表、TLB/cache
  platform/qemu_virt/ # 地址布局、GIC、架构 timer、PSCI
  kernel/
    boot.rs          # 初始资源与 root task
    object/          # TCB、CNode、Untyped、Frame 等
    capability.rs
    scheduler.rs
    syscall.rs
    ipc.rs
    fault.rs
  debug/             # LOG 控制的日志与轮询 UART
crates/
  abi/               # no_std 常量、固定布局结构、错误码
  user-runtime/      # CRT、SVC wrappers、基础用户库
user/
  fatboot/
  serial-server/
  hello/
tools/
  boot-image/        # 宿主打包
  disk-image/        # FAT32 镜像制作
tests/
  host/              # 位域、解析器、能力状态机
  qemu/              # 启动、隔离、调度、IPC、端到端
docs/
  microkernel-design.md
```

内核和用户程序分别使用链接脚本。Cargo workspace 不能把当前内核全局 `-Tlink.lds` 自动套给全部用户 ELF；按 package/build script 或独立构建目标选择链接参数。`abi` 不依赖内核内部对象；跨边界结构采用 `repr(C)`、固定宽度类型、显式版本和保留字段，不直接暴露 Rust enum、Vec、引用或内部指针。

syscall 寄存器约定建议统一为 x8 操作号、x0 目标 CapPtr/返回状态、x1…x6 标量参数与结果，其余寄存器默认保存。IPC 具体占用的消息寄存器需在 IPC 与故障 固化；SVC 包装正确声明寄存器和内存影响。该约定是本项目设计，不是 seL4 ABI。

所有 syscall 必须定义非法 capability、权限不足、类型不符、参数错误、内存不足、不支持、对端消失等错误。用户内存访问不能直接把任意地址转为 Rust 引用；实现有边界检查和 fault 恢复能力的复制，或使用已验证、受控映射的 IPC buffer。

## 17. 验证方式与阶段退出条件

| 验证层 | 主要检查 | 适用阶段 |
| --- | --- | --- |
| 宿主测试 | Cap 派生/撤销状态机、尺寸溢出、ELF/FAT 解析异常输入 | 能力与对象、完整 fatboot 起 |
| 构建检查 | 内核无存储/网络依赖、链接布局、debug 消除 | 构建基线 起 |
| QEMU 集成 | EL0、访问权限、抢占、跨 VSpace、IPC 故障 | 首个用户任务 起 |
| 端到端 | 无盘串口、磁盘 hello、替换磁盘程序、失败恢复 | 用户态串口 起 |

每个阶段需要正常用例和对应的权限/失败用例；不能只观察一行 hello。调度测试要使用不主动让出的忙循环；能力测试要验证撤销后的旧引用；进程测试要验证同 VA 的不同物理内容。

release 且LOG=off 时，早期用 GDB 断点/结果页验证启动；用户态串口 起用用户串口协议报告结果。测试 harness 设置超时，区分“预期暂停”“panic 停机”“异常死循环”。只有带 kernel-test 的镜像可以提供测试退出通道，正式构建不保留无授权的关机 syscall。

从 首个用户任务 开始持续检查：

- 用户不可读写内核页，不可使用未授权 MMIO，不可自行安装页表。
- 用户可控错误不得成为内核 panic；可恢复的内核用户复制 fault 有明确恢复路径。
- 对象引用有效，线程至多在一个与状态匹配的队列中，回复权限不可重放。
- 未清零、未撤销映射、未停止 DMA 的资源不得重新发放。
- debug 配置变化不改变正常 syscall 权限语义和用户服务可用性。

## 18. 第一轮实施任务清单

建议先只提交 构建基线/内核底座，再做 首个用户任务；不要同一轮把磁盘、调度、IPC 和能力系统全部引入。

- [x] 修正构建产物名，提供无盘无网启动目标。
- [x] 将 FAT/分配器自测从默认启动路径移出。
- [x] 普通输出统一使用 log 宏和 LOG 等级，panic 保留独立应急输出。
- [x] 明确启动 EL、栈、BSS、页表对齐、RAM 管理边界。
- [x] 划分内核代码/数据权限，收缩设备映射。
- [x] 区分用户与内核异常，补充 ESR/FAR 诊断与终止策略。
- [x] 为后续用户 ELF 增加独立 linker/CRT/ABI crate。
- [x] 打包最小 fatboot，构造 BootInfo、用户页表和初始任务上下文（尚无 CSpace）。
- [x] 验证 EL0 → SVC → EL0 与用户非法访问。

到这里，才完成“先把内核底座收干净，再启动第一个用户程序”的第一轮目标。其余功能按 能力与对象 → 完整 fatboot 顺序推进。

## 19. 参考与核对入口

本地源码为本文的主要事实依据；以下路径以当前项目根目录为基准：

| 主题 | 参考路径 |
| --- | --- |
| seL4 ARM 初始化、首线程、BootInfo、Untyped | `../seL4/kernel/src/arch/arm/kernel/boot.c` |
| 通用启动对象构造 | `../seL4/kernel/src/kernel/boot.c` |
| capability 查找与管理 | `../seL4/kernel/src/kernel/cspace.c`、`src/object/cnode.c` |
| Untyped 创建 | `../seL4/kernel/src/object/untyped.c` |
| 调度与线程 | `../seL4/kernel/src/kernel/thread.c`、`src/object/tcb.c` |
| IPC 与通知 | `../seL4/kernel/src/object/endpoint.c`、`src/object/notification.c` |
| 故障与 IRQ | `../seL4/kernel/src/kernel/faulthandler.c`、`src/object/interrupt.c` |
| debug/printing 配置 | `../seL4/kernel/config.cmake` 的 KernelDebugBuild/KernelPrinting |
| 本地 rootserver 声明 | `../seL4/projects/sel4test/apps/CMakeLists.txt` |
| fatboot 用户态启动流程 | `../seL4/projects/sel4test/apps/boot/main.c` |
| 串口服务与协议 | `../seL4/projects/sel4test/apps/serial/main.c`、`apps/include/console.h` |

seL4 本身将 `KernelDebugBuild` 与 `KernelPrinting` 分开配置，后者默认值跟随前者。本项目按需求使用单一 LOG 等级控制日志，未复刻这两个配置；不能反过来当作 seL4 全部配置的描述。

官方资料补充核对了三个概念：首个用户程序是 root task，由启动期提供镜像和 BootInfo；capability 是访问资源的授权；Untyped 是用户态请求构造对象的内存来源。参见 [Rust root task 教程](https://docs.sel4.systems/projects/rust/tutorial/root-task/)、[Capabilities 教程](https://docs.sel4.systems/Tutorials/capabilities.html)、[Untyped 教程](https://docs.sel4.systems/Tutorials/untyped.html)。本文的阶段划分、ABI、日志交接与首版限制均为面向本项目的设计选择。
