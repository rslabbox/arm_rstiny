# Rust bootloader 引导链

本项目使用 `bootloader/` 中的 `no_std` Rust 引导程序，保留此前 seL4 ARM elfloader 的 CPIO 格式、镜像布局和六寄存器交接协议。用户程序通过链接段嵌入 bootloader，不嵌入内核 ELF。

## 上游来源与构建

实现及构建契约见 [bootloader/README.md](../bootloader/README.md)。程序使用有界 CPIO/ELF 解析、无堆分配的装载计划、类型化 PL011 寄存器和 Rust 内联汇编。

构建工具生成归档后调用 Cargo；`bootloader/build.rs` 用 `.incbin` 将其放入 `.boot_archive` 只读段，`bootloader/linker.ld` 定义布局。入口与 MMU 操作使用必要的 naked/inline asm，没有独立汇编源文件。

需要 GNU `cpio`、device-tree-compiler（`dtc`/`fdtget`）、QEMU AArch64、Python 3、Rust nightly 和 cargo-binutils/LLVM tools。运行 `make build` 或 `make run LOG=info`：

1. `tools/build_platform.py` 按固定的 virtualization=off 配置导出 QEMU DTB，转换为 DTS 并重新编译以去除预留填充，生成 `kernel.dts/kernel.dtb`，不使用 overlay。
2. 构建时通过 `fdtget` 读取最终 DTB：按 compatible 属性选择当前平台设备；生成内核与 bootloader 使用的 `platform.rs` 及可读的 `platform.json`。
3. 先编译 hello，strip 后得到 `hello.elf`，通过 `HELLO_ELF` 链接到 fatboot 的只读段。编译 EL1 内核，PSCI HVC 在构建时从 DTB 确定。独立编译 fatboot。校验 ELF64/AArch64 段、入口、W^X、栈和物理范围，生成去除调试信息的副本。
4. 按 `kernel.elf`、`kernel.dtb`、`rootserver` 的顺序生成 newc CPIO，固定归档时间和所有者。`rootserver` 是 fatboot ELF。
5. 通过 `BOOT_ARCHIVE` 和 `PLATFORM_DIR` 构建 Rust bootloader，将 CPIO 链接进 ELF；QEMU 的 `-kernel` 指向这个 ELF。

设备树产物位于 `target/platform/qemu-arm-virt/`；原始导出为 `qemu-arm-virt.dtb/dts`，用于打包的紧凑版本为 `kernel.dtb/dts`。生成脚本、QEMU 或 dtc 版本变化会使缓存失效；生成器检查 UART、GICv3、物理定时器、PSCI HVC 和单核 RAM 布局符合当前固定平台。

产物为 `target/kernel/<MODE>-log<LOG>-test<0|1>/image/bootloader`，其 Cargo 中间产物位于同级 `build/`。平台固定为 `virt,gic-version=3,virtualization=off`、Cortex-A72、单核、128 MiB RAM，只构建一套内核和 loader。`kernel.bin` 仅为辅助产物，不能代替 bootloader 直接启动。

## 物理布局与交接

loader 位于物理地址 `0x44000000`。内核物理起点为 `0x40200000`，虚拟起点为 `0xffff000040200000`，VA − PA 固定为 `0xffff000000000000`。loader 在内核内存末尾放置 DTB，再按页对齐装载 root ELF，随后保留一页用户程序头。构建器及 loader 检查这些内容不超过 `0x42000000`，全部输入验证完成后才写入目的 RAM。

loader 负责复制 PT_LOAD、清零 BSS 和 ELF 栈、建立临时页表并开启 MMU/cache。本项目选择非 hypervisor 内核配置，QEMU 从 EL1 启动，loader 交接到 EL1 内核。交接寄存器与上游非 hypervisor AArch64 路径一致：

| 寄存器 | 含义 |
| --- | --- |
| x0 | 用户镜像物理起点 |
| x1 | 用户镜像物理终点，不包含后面的程序头页 |
| x2 | 用户镜像物理地址减虚拟地址的偏移 |
| x3 | 用户 ELF 虚拟入口 |
| x4 | DTB 物理地址 |
| x5 | DTB 字节数 |

Rust 内核保存并校验参数，然后以 TTBR1 高地址页表替换 loader 临时内核映射，TTBR0 用于独立用户地址空间。运行期内核 text RX、rodata R/NX、其他 RAM RW/NX，UART/GIC 为 Device/NX；保留内核栈保护页。bootloader ELF 分为 RX、R、RW 段；其临时页表以 1 GiB 块映射设备和 RAM，RAM 引导映射允许 EL1 读写执行，由内核正式页表收紧权限。

## root task 初始化

内核接管 root image 的原始物理页，不再复制用户段。loader 保留的程序头页包含两个 u32（程序头数量和每项大小）以及 ELF 程序头数组；本项目读取这些现有元数据，按 PT_LOAD 权限建立用户映射。保留 W^X 是本项目的映射策略，不意味着 seL4 内核也逐段采取相同权限策略。

fatboot 的 ELF 虚拟范围为 `0x400000..0x600000`，其中 `0x5fc000..0x600000` 为运行库拥有的 NOLOAD 栈段。IPC buffer 紧接镜像末尾，位于 `0x600000`；BootInfo 在下一页 `0x601000`。首次进入 EL0 时 x0 指向 BootInfo，其余通用寄存器和 SP 为零；`#[entry]` 生成的入口从 `__user_stack_top` 设置 SP，再进入 Rust。

原有 8 MiB 帧池继续负责分配，另接管 2 MiB root image 物理区间。root 持有已映射页，未映射空洞在接管完成后可分配，任务释放后页面可复用。DTB 和程序头页保持保留，尚未实现全 RAM 发现与回收。

PSCI 的 SMC/HVC 在构建时读取 DTB 的 `psci/method` 并生成常量，不能由内核入口 EL 推断。内核 `LOG` 控制标准日志和临时用户 debug 输出，panic 独立打印后关机；bootloader 的启动信息不受内核 `LOG` 控制。

内核不解析 DTB 内容，只验证 loader 交接的物理范围与大小，并将原始字节复制到用户只读的扩展 BootInfo。fatboot 不解析或检查设备树，仅负责加载 hello；扩展 BootInfo 的 DTB 传递接口继续保留。

BootInfo ABI 版本为 2，结构为 96 字节，新增 extra 指针和 extra_size。扩展区从 `0x602000` 开始，按页映射并固定；首个记录采用两个 u64 的 id/len 头（FDT id=6，len 包含 16 字节头），后接完整 DTB。这是参考 seL4 的扩展记录方式，仍不兼容其整体 BootInfo 二进制布局。运行库校验记录边界后，以 `BootInfo::device_tree()` 提供只读切片。

默认 GICv3 平台由 `arm-gic-driver 0.17.13` 管理中断控制器，内核映射 Distributor 的 `0x08000000..0x08010000` 和单核 Redistributor 的 `0x080a0000..0x080c0000`，CPU 接口通过系统寄存器访问。调度定时器使用 Group 1 的 INTID 30，先重装定时器再 EOI；EOI 同时完成优先级下降和 deactivate。特殊/伪中断 ID 不执行 EOI，暂未授权的其他中断会被屏蔽。QEMU 运行参数与归档 DTB 同步选择 GICv3。

## 一致性边界与验证

Rust bootloader 是本项目的实现，沿用此前 seL4 elfloader 的归档约定、镜像放置、程序头保留页及六寄存器交接协议；不宣称具备上游的全部平台支持。构建驱动、链接脚本、平台固定地址、Rust 内核运行期内存策略属于本项目；BootInfo 内容、系统调用、任务句柄也仍为本项目 ABI，尚未实现 seL4 的完整 capability、Untyped 和 IPC，不能直接运行 seL4 用户二进制。

`tools/check_bootloader.py` 在真实 QEMU 中破坏 CPIO、ELF 和 DTB，验证 debug/release 均在写入目标 RAM 前打印错误并停在 `bootloader_halt`，不进入内核。入口要求 EL1、MMU/cache 关闭；不符合入口条件时在使用栈和串口前停机。

`make check` 包含 ELF 输入校验单元测试、宏测试和 QEMU 集成测试。平台宿主测试检查 原生 DTB 的设备选择、缓存重建和 PSCI HVC 配置。内核入口断点逐项核对六个交接参数、EL1/MMU/cache 状态、装载字节、BSS/栈清零及保留程序头。EL0 测试检查原地物理映射、运行库设置栈、BootInfo、DTB 逐字节传递及零填充、只读权限和故障隔离；内存与任务测试继续检查回收、耗尽回滚、所有权、定时器抢占、等待与唤醒。覆盖 debug/release、LOG=off/info ，固定 EL1 入口，日志测试仅在 loader 交接标记之后检查内核输出。
