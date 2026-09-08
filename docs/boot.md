# Rust bootloader 引导链

本项目使用 `bootloader/` 中的 `no_std` Rust 引导程序，保留此前 seL4 ARM elfloader 的 CPIO 格式、镜像相对顺序和六寄存器交接协议。用户程序通过链接段嵌入 bootloader，不嵌入内核 ELF。

## 上游来源与构建

实现及构建契约见 [bootloader/README.md](../bootloader/README.md)。程序使用有界 CPIO/ELF 解析、无堆分配的装载计划、类型化 PL011 寄存器和 Rust 内联汇编。

构建工具生成 CPIO 后，用 `rust-objcopy` 转成包含 `.boot_archive` 只读段的 AArch64 目标文件 `archive.o`，再调用 Cargo；`bootloader/build.rs` 将该目标文件交给链接器，`bootloader/linker.ld` 定义布局。入口与 MMU 操作使用必要的 naked/inline asm，没有独立汇编源文件。

需要 GNU `cpio`、device-tree-compiler（`dtc`/`fdtget`）、QEMU AArch64、Python 3、Rust nightly 和 cargo-binutils/LLVM tools。运行 `make build` 或 `make run LOG=info`：

1. `tools/build_platform.py` 按固定的 virtualization=off 配置导出 QEMU DTB，转换为 DTS 并重新编译以去除预留填充，生成 `kernel.dts/kernel.dtb`，不使用 overlay。
2. 构建时通过 `fdtget` 读取最终 DTB：按 compatible 属性选择当前平台设备；生成内核使用的 `platform.rs` 及可读的 `platform.json`。
3. 先编译 hello，strip 后得到 `hello.elf`，通过 `HELLO_ELF` 链接到 fatboot 的只读段。编译 EL1 内核，PSCI HVC 在构建时从 DTB 确定。独立编译 fatboot。校验 ELF64/AArch64 段、入口、W^X、栈和物理范围，生成去除调试信息的副本。
4. 按 `kernel.elf`、`kernel.dtb`、`rootserver` 的顺序生成 newc CPIO，固定归档时间和所有者。`rootserver` 是 fatboot ELF。
5. 通过 `BOOT_ARCHIVE_OBJECT` 构建 Rust bootloader，将 CPIO 链接进 ELF；QEMU 的 `-kernel` 指向这个 ELF。

设备树产物位于 `target/platform/qemu-arm-virt/`；原始导出为 `qemu-arm-virt.dtb/dts`，用于打包的紧凑版本为 `kernel.dtb/dts`。生成脚本、QEMU 或 dtc 版本变化会使缓存失效；生成器检查 UART、GICv3、物理定时器、PSCI HVC 和单核 RAM 布局符合当前固定平台。

产物为 `target/kernel/<MODE>-log<LOG>-test<0|1>/image/bootloader`，其 Cargo 中间产物位于同级 `build/`。平台固定为 `virt,gic-version=3,virtualization=off`、Cortex-A72、单核、128 MiB RAM，只构建一套内核和 loader。`kernel.bin` 仅为辅助产物，不能代替 bootloader 直接启动。

## 物理布局与交接

loader 位于物理地址 `0x44000000`。内核链接在 `0xffff800000000000` 的独立高地址窗口，ELF 的 `p_paddr` 不决定实际装载位置。loader 从 128 MiB RAM 中排除前 2 MiB 固件区域和自身完整范围（代码、归档、页表、栈），以 2 MiB 对齐寻找能容纳整个镜像组的首个空闲区间。DTB 紧随内核，再按页对齐放置 rootserver 和一页保留程序头。候选空间不足时继续搜索下一个空闲区间，所有校验完成后才写入目的 RAM。

loader 负责复制 PT_LOAD、清零 BSS 和 ELF 栈、建立临时页表并开启 MMU/cache。本项目选择非 hypervisor 内核配置，QEMU 从 EL1 启动，loader 交接到 EL1 内核。交接寄存器与上游非 hypervisor AArch64 路径一致：

| 寄存器 | 含义 |
| --- | --- |
| x0 | 用户镜像物理起点 |
| x1 | 用户镜像物理终点，不包含后面的程序头页 |
| x2 | 用户镜像物理地址减虚拟地址的偏移 |
| x3 | 用户 ELF 虚拟入口 |
| x4 | DTB 物理地址 |
| x5 | DTB 字节数 |

Rust 内核通过 `AT S1E1R`/`PAR_EL1` 查询自身首地址的物理映射，保存该基址并校验参数，然后以 TTBR1 高地址页表替换 loader 临时内核映射，TTBR0 用于独立用户地址空间。运行期内核 text RX、rodata R/NX、其他 RAM RW/NX，UART/GIC 为 Device/NX；保留内核栈保护页。bootloader ELF 分为 RX、R、RW 段；其临时页表以 1 GiB 块映射恒等地址及物理直接映射，以 2 MiB 块独立映射内核镜像；RAM 引导映射允许 EL1 读写执行，由内核正式页表收紧权限。

## root task 初始化

内核接管 root image 的原始物理页，不再复制用户段。loader 保留的程序头页包含两个 u32（程序头数量和每项大小）以及 ELF 程序头数组；本项目读取这些现有元数据，按 PT_LOAD 权限建立用户映射。保留 W^X 是本项目的映射策略，不意味着 seL4 内核也逐段采取相同权限策略。

fatboot 的镜像边界由 ELF 的 PT_LOAD 段推导；运行时入口宏在 BSS 中保留按页对齐的栈及保护页存储。内核不约定用户镜像或栈地址。IPC buffer 紧接按页取整的镜像末尾，BootInfo 在下一页，扩展区随后。首次进入 EL0 时 x0 指向 BootInfo，其余通用寄存器和 SP 为零；`#[entry]` 生成的入口从 `__user_stack_top` 设置 SP，随后由用户运行时撤销保护页映射，再进入应用 Rust main。

原有 8 MiB 帧池继续负责分配，另接管 ELF 实际跨度对应的 root image 物理区间（最多 1024 页）。root 持有已映射页，未映射空洞在接管完成后可分配，任务释放后页面可复用。DTB 和程序头页保持保留，尚未实现全 RAM 发现与回收。

PSCI 的 SMC/HVC 在构建时读取 DTB 的 `psci/method` 并生成常量，不能由内核入口 EL 推断。内核 `LOG` 控制标准日志和临时用户 debug 输出，panic 独立打印后关机；bootloader 的启动信息不受内核 `LOG` 控制。

内核不解析 DTB 内容，只验证 loader 交接的物理范围与大小，并将原始字节复制到用户只读的扩展 BootInfo。fatboot 不解析或检查设备树，仅负责加载 hello；扩展 BootInfo 的 DTB 传递接口继续保留。

BootInfo ABI 版本为 3，结构为 64 字节，仅发布启动元信息、IPC buffer 和扩展区指针及长度，不发布镜像或栈边界。扩展区紧接 BootInfo 页，按页映射并固定；首个记录采用两个 u64 的 id/len 头（FDT id=6，len 包含 16 字节头），后接完整 DTB。这是参考 seL4 的扩展记录方式，仍不兼容其整体 BootInfo 二进制布局。运行库校验记录边界后，以 `BootInfo::device_tree()` 提供只读切片。

默认 GICv3 平台由 `arm-gic-driver 0.17.13` 管理中断控制器，内核映射 Distributor 的 `0x08000000..0x08010000` 和单核 Redistributor 的 `0x080a0000..0x080c0000`，CPU 接口通过系统寄存器访问。调度定时器使用 Group 1 的 INTID 30，先重装定时器再 EOI；EOI 同时完成优先级下降和 deactivate。特殊/伪中断 ID 不执行 EOI，暂未授权的其他中断会被屏蔽。QEMU 运行参数与归档 DTB 同步选择 GICv3。

## 一致性边界与验证

Rust bootloader 是本项目的实现，沿用此前 seL4 elfloader 的归档约定、镜像相对顺序、程序头保留页及六寄存器交接协议；不宣称具备上游的全部平台支持。动态物理选址、独立内核虚拟窗口、启动映射查询以及 Rust 内核运行期内存策略属于本项目；BootInfo 内容、系统调用、任务句柄也仍为本项目 ABI，尚未实现 seL4 的完整 capability、Untyped 和 IPC，不能直接运行 seL4 用户二进制。

`tools/check_bootloader.py` 在真实 QEMU 中破坏 CPIO、ELF 和 DTB，验证 debug/release 均在写入目标 RAM 前打印错误并停在 `bootloader_halt`，不进入内核。入口要求 EL1、MMU/cache 关闭；不符合入口条件时在使用栈和串口前停机。

`make check` 包含 ELF 输入校验单元测试、宏测试和 QEMU 集成测试。平台宿主测试检查 原生 DTB 的设备选择、缓存重建和 PSCI HVC 配置。内核入口断点逐项核对六个交接参数、EL1/MMU/cache 状态、装载字节、BSS/栈清零及保留程序头。EL0 测试检查原地物理映射、运行库设置栈、BootInfo、DTB 逐字节传递及零填充、只读权限和故障隔离；内存与任务测试继续检查回收、耗尽回滚、所有权、定时器抢占、等待与唤醒。覆盖 debug/release、LOG=off/info ，固定 EL1 入口，日志测试仅在 loader 交接标记之后检查内核输出。

## 共享 ELF 解析

`projects/libs/elf`（`rstiny-elf`）由 Rust bootloader 和用户态 ELF loader 共用，
借鉴 x-kernel 的解析/装载分层及分步读取接口，未引入其实现或依赖。
库为 `no_std`，不使用 `alloc` 或裸指针转换；借用程序头字节，通过迭代器解码段，
不在启动栈或用户栈上保存段数组。`Header::parse` 可先读取 64 字节文件头，
随后按 `program_headers_range()` 读取程序头表，使用 `Elf::from_headers` 和文件长度校验；
完整内存镜像则使用 `Elf::parse`。分步读取者必须确保元数据和后续段数据来自同一未改变的文件。

当前明确支持静态、小端 AArch64 ELF64、最多 32 个程序头及 4 KiB 页对齐装载段，
不支持动态链接或 TLS。解析器以具名错误报告截断、溢出、段越界、页面重叠和无效入口，
校验 `filesz <= memsz`（含空段）；保留原始程序头表供 seL4 风格交接使用。
W^X、平台地址窗口和装载目标由调用方检查，内存复制仍在 bootloader 或用户态装载器完成。
内核不依赖此通用文件解析库；BootInfo、rootserver 栈和系统调用 ABI 未改变。

`make check` 包含该库的宿主测试：非对齐输入、分步读取、截断、地址溢出、
非法段/入口以及重叠映射；现有 QEMU 启动和用户任务测试覆盖两个装载器的集成。

## 启动归档类型

`bootloader/src/archive.rs` 将 newc 解码和启动镜像协议分开：内部 `Cursor`
负责有界读取和四字节对齐，`NewcHeader` 解码 ASCII 十六进制字段，
`Entry` 借用文件名和内容，`Record` 区分普通文件与结束记录。
对外 `BootArchive::parse` 验证 `kernel.elf`、`kernel.dtb`、`rootserver` 的固定顺序，
通过具名访问器提供内容；构造成功不代表 ELF/DTB 内容已经验证。

解析无堆分配，拒绝内部 NUL 文件名、非法头字段、错误文件顺序及非空 Trailer。
Trailer 后仅允许零填充。`ArchiveError` 保留错误类型和归档内字节偏移，
启动失败时串口输出该位置。`make check` 通过 Cargo 的 `images` 宿主测试目标编译纯解析和规划模块，
无需 bootloader 链接脚本或启动镜像；QEMU 测试验证拒绝路径不会写入目标 RAM。

## 临时页表与 MMU 交接

bootloader 的 `enter` 依次调用 `mmu::init_boot_page_tables`、`mmu::enable_mmu`
和 `jump_to_kernel`。`bootloader/src/mmu.rs` 参考内核的页表项封装，以
`PageTableEntry::table/block` 和具名描述符属性构造静态页表；通过
`aarch64-cpu` 的具名 MAIR/TCR/SCTLR 字段配置寄存器，保留缓存维护、TLB 失效
和屏障顺序。无动态分配，页表初始化只允许在启动 CPU、MMU 关闭时执行一次。

TTBR0 保留 loader 的低地址恒等映射；TTBR1 包含固定偏移的物理直接映射和独立内核镜像映射。
前两者以 1 GiB 块映射物理 0..1 GiB 为 Device/XN，1..2 GiB 为普通 RAM；
镜像映射以 2 MiB 块将 ELF 的虚拟地址映射到动态分配的物理区间，均禁止 EL0 访问。
当前 PC/SP 位于恒等映射中，因此开启 MMU 后仍有效。
MAIR 保持原有 `0x0000aaff440c0400` 布局，槽 0 为 Device，槽 4 为普通 WB 内存。
内核随后用自己的细粒度页表替换临时映射；六寄存器交接 ABI 不变。

## bootloader 源码职责

- `main.rs`：模块声明与启动流程编排。
- `entry.rs`：裸入口、启动条件检查、栈设置和 BSS 清零。
- `image.rs`：`BootImages` 解析、`LoadPlan` 校验、显式装载与交接参数构造。
- `layout.rs`：物理范围、虚拟/物理映射类型及排除保留区的无堆分配算法。
- `device_tree.rs`：DTB 头部与范围校验视图。
- `boot_info.rs`：已装载镜像的交接描述。
- `handoff.rs`：MMU 初始化编排和六寄存器跳转。
- `mmu.rs`：临时页表和 MMU/缓存配置。
- `console.rs`：串口格式化输出、panic 和失败停驻。
- `archive.rs`、`elf.rs`、`pl011.rs`：归档解析、启动 ELF 策略和 UART 寄存器操作。

启动入口中的异常级和 MMU/cache 检查在设置 SP 后由 `entry::check_boot_context`
通过 `aarch64_cpu` 执行，早于 BSS 清零及串口初始化；裸汇编只负责中断屏蔽、
启动 CPU 筛选和栈设置。非启动 CPU 仍在使用栈前停驻。启动协议必须保证代码和
链接地址处的启动栈可访问，Rust 检查无法保护此前的首次栈访问；不满足 EL1、
MMU/cache 关闭条件时静默停驻，未新增 EL2/EL3 切换支持。

## 归档目标文件构建

`make build` 是完整入口：`build_image.py` 按固定文件名和顺序创建 newc CPIO，
统一归档输入的权限、时间戳及所有者，再通过 `rust-objcopy` 的 binary 输入模式生成
AArch64 可重定位目标文件 `archive.o`。该目标文件的 `.boot_archive` 段只读、
不可执行、四字节对齐；链接脚本保留该段并定义起止符号。

归档和目标文件内容未变化时保留原文件；objcopy 使用固定输入文件名，目标文件
内容不依赖构建目录。`BOOT_ARCHIVE_OBJECT` 传入绝对目标文件路径，Cargo 的
`rerun-if-changed` 跟踪它，内容更新触发重新链接。无需生成 Rust 源码或汇编嵌入指令。
缺少归档时，check/clippy 仍可进行源码检查，但实际链接由 ASSERT 拒绝。
`tools/test_boot_archive.py` 验证目标文件属性、跨目录可复现性、同路径同长度内容
变化后的实际重新链接，以及缺少归档的链接失败。

bootloader 的固定 UART 地址直接定义于 `bootloader/src/platform.rs`，不再包含生成的
平台 Rust 文件，也不需要 `allow(dead_code)`。`build.rs` 仅处理归档目标文件和链接脚本。
完整镜像流程仍通过 `build_platform.py` 生成、验证 DTB，内核继续使用其生成的平台配置；
UART 和 128 MiB RAM 布局一致性在该构建步骤检查，bootloader 保留实际镜像装载范围校验。

## 动态物理装载与内核映射

正常入口仍为 `make build` / `make run`。默认寻找第一个足够大的空闲区间，
因此当前镜像通常落在 `0x40200000`，但它不是内核要求的物理地址。
`make run KERNEL_LOAD_MIN=0x41000000` 将搜索下界移到指定地址；这只影响 loader 的
选址策略，不重新链接内核。如果下界附近空间被 loader 占用，分配器会跳过整个保留区。
超过 RAM 或没有足够连续空间会在任何目标写入之前失败。该参数用于验证和部署布局，
并不代表支持动态虚拟基址随机化或基于任意 DTB 的通用物理内存管理。

内核镜像窗口为 `0xffff800000000000..0xffff800002000000`（32 MiB），
直接映射使用固定偏移 `0xffff000000000000`。内核在 BSS 清零后，通过仍然有效的
loader 页表查询 `skernel` 的 PA，记录于内部 `LOADER_BOOT_INFO.kernel_physical`；
不会增加 x0..x5 的参数或改变用户态 BootInfo ABI。

正式 TTBR1 为镜像和直接映射分别建表：镜像 text RX、rodata R/NX、其余 RW/NX；
对应直接映射别名一律 NX，text/rodata 别名也只读，避免可写别名绕过 W^X。
内核栈保护页在两处均不映射。物理帧分配器统一保存直接映射地址，页表与堆的
镜像地址则通过运行时基址转换。UART/GIC 地址不随内核物理位置移动。
直接映射页表可描述整个 128 MiB RAM 窗口，但只映射已接管的镜像区间和必要设备；
运行时可分配帧池容量保持原有约束，没有顺带扩大资源授权范围。

`make check` 包含 `check_relocation.py`：对 debug/release 的同一内核 ELF 比较哈希，
在 loader 下方和上方分别启动，验证六寄存器参数、内核实际拷贝、BSS、两类映射权限、
hello 执行和任务内存回收；还验证无可用空间时的拒绝路径。

## 用户布局独立性

root 链接布局使用 LLD 默认规则，由 `tools/build_app.py` 统一设置页对齐等参数，可通过 `ROOT_IMAGE_BASE` 构建参数覆盖；loader 和内核从 ELF/交接范围推导启动页位置。零页禁止映射，当前用户地址上限为 128 MiB，镜像跨度最多 1024 页，实际映射页（含启动元数据）同样受每任务 1024 页配额约束。这些是资源边界，不是固定用户布局。

`check_relocation.py` 在 debug/release 下还验证两种 root 链接地址，逐页检查权限和 BootInfo，启动 hello 并运行内存/调度测试，同时确认 kernel ELF 哈希不变。
