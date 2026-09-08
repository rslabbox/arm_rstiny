# seL4 ARM elfloader 引导链

本项目直接编译 `../seL4` 中的 ARM elfloader 原版源码，保持其 ELF 装载、CPIO 格式、链接脚本和内核交接协议。用户程序不嵌入内核 ELF；已删除自定义启动模块打包器和 `ROOT_IMAGE` 构建依赖。

## 上游来源与构建

`loader/vendor/elfloader` 来自 seL4 tools 的 elfloader-tool，tools commit 为 `7dd5ba144b1fecf1358a12d2bef3eb365aab35c7`。`loader/vendor/libcpio` 来自 util_libs commit `6e55b3c62687779692150e1de411ce61b9d2919a`。来源记录为相邻 seL4 工作树，逐文件 SHA-256 保存在 [upstream.json](../loader/upstream.json)，每次构建都会校验。vendor 源码未修改，保留 SPDX 标识及许可证；平台配置放在 `loader/config`。

本项目用 Python 调用 AArch64 GCC，替代上游整套 CMake 工程集成；实际编译的 loader、libcpio 代码和预处理链接脚本仍为原版。源码清单见 `loader/sources.json`。这部分包含上游 C 和汇编，内核自身的启动入口继续放在 Rust 中。

需要 `aarch64-linux-gnu-gcc`、GNU `cpio`、QEMU AArch64、Python 3、Rust nightly 和 cargo-binutils/LLVM tools。运行 `make build` 或 `make run LOG=info`：

1. 分别链接 kernel 和 fatboot，保留带符号 ELF 供调试。
2. 校验 ELF64/AArch64 静态装载段、入口、W^X、用户栈及平台物理范围，生成去除调试信息的副本。
3. QEMU 根据对应的 virtualization 配置生成 DTB；配置或 QEMU 版本变化会重新生成。
4. 按 `kernel.elf`、`kernel.dtb`、`rootserver` 的顺序生成 newc CPIO，固定归档时间和所有者。`rootserver` 是 fatboot ELF。
5. 将 CPIO 放入 `._archive_cpio`，由原版 `src/linker.lds` 链接进 elfloader。QEMU 的 `-kernel` 指向这个 ELF。

产物为 `target/kernel/<MODE>-log<LOG>-test<0|1>/image/{el2,el1}/elfloader`。默认使用 el2 版本，对应 `virt,gic-version=3,virtualization=on`；el1 版本对应 `virtualization=off`。两者固定为 Cortex-A72、单核、128 MiB RAM。`kernel.bin` 仅为辅助产物，不能代替 elfloader 直接启动。

## 物理布局与交接

loader 位于物理地址 `0x44000000`。内核物理起点为 `0x40200000`，虚拟起点为 `0xffff000040200000`，VA − PA 固定为 `0xffff000000000000`。上游 loader 在内核内存末尾放置 DTB，再按页对齐装载 root ELF，随后保留一页用户程序头。构建器检查这些内容不超过 `0x42000000`。

loader 负责复制 PT_LOAD、清零 BSS 和 ELF 栈、建立临时页表并开启 MMU/cache。本项目选择非 hypervisor 内核配置，固件从 EL1 或 EL2 启动都交接到 EL1 内核。交接寄存器与上游非 hypervisor AArch64 路径一致：

| 寄存器 | 含义 |
| --- | --- |
| x0 | 用户镜像物理起点 |
| x1 | 用户镜像物理终点，不包含后面的程序头页 |
| x2 | 用户镜像物理地址减虚拟地址的偏移 |
| x3 | 用户 ELF 虚拟入口 |
| x4 | DTB 物理地址 |
| x5 | DTB 字节数 |

Rust 内核保存并校验参数，然后以 TTBR1 高地址页表替换 loader 临时内核映射，TTBR0 用于独立用户地址空间。运行期内核 text RX、rodata R/NX、其他 RAM RW/NX，UART/GIC 为 Device/NX；保留内核栈保护页。loader 自己的临时 ELF 布局沿用上游，会产生 RWX LOAD segment 链接提示；它与内核运行期的 W^X 映射是两个不同阶段。

## root task 初始化

内核接管 root image 的原始物理页，不再复制用户段。上游 loader 保留的程序头页包含两个 u32（程序头数量和每项大小）以及 ELF 程序头数组；本项目读取这些现有元数据，按 PT_LOAD 权限建立用户映射。保留 W^X 是本项目的映射策略，不意味着 seL4 内核也逐段采取相同权限策略。

fatboot 的 ELF 虚拟范围为 `0x400000..0x600000`，其中 `0x5fc000..0x600000` 为运行库拥有的 NOLOAD 栈段。IPC buffer 紧接镜像末尾，位于 `0x600000`；BootInfo 在下一页 `0x601000`。首次进入 EL0 时 x0 指向 BootInfo，其余通用寄存器和 SP 为零；`#[entry]` 生成的入口从 `__user_stack_top` 设置 SP，再进入 Rust。

原有 8 MiB 帧池继续负责分配，另接管 2 MiB root image 物理区间。root 持有已映射页，未映射空洞在接管完成后可分配，任务释放后页面可复用。DTB 和程序头页保持保留，尚未实现全 RAM 发现与回收。

PSCI 的 SMC/HVC 选择读取 DTB 的 `psci/method`，不能由内核入口 EL 推断。内核 `LOG` 控制标准日志和临时用户 debug 输出，panic 独立打印后关机；原版 elfloader 的启动信息不受内核 `LOG` 控制。

默认 GICv3 平台由 `arm-gic-driver 0.17.13` 管理中断控制器，内核映射 Distributor 的 `0x08000000..0x08010000` 和单核 Redistributor 的 `0x080a0000..0x080c0000`，CPU 接口通过系统寄存器访问。调度定时器使用 Group 1 的 INTID 30，先重装定时器再 EOI；EOI 同时完成优先级下降和 deactivate。特殊/伪中断 ID 不执行 EOI，暂未授权的其他中断会被屏蔽。QEMU 运行参数与归档 DTB 同步选择 GICv3，原版 loader 本身无需修改。

## 一致性边界与验证

与 seL4 一致的范围是原版 ARM elfloader、归档及链接方式、ELF 装载行为和六寄存器交接协议。构建驱动、平台固定地址、Rust 内核运行期内存策略属于本项目；BootInfo 内容、系统调用、任务句柄也仍为本项目 ABI，尚未实现 seL4 的完整 capability、Untyped 和 IPC，不能直接运行 seL4 用户二进制。

`make check` 包含 ELF 输入校验单元测试、宏测试和 QEMU 集成测试。内核入口断点逐项核对六个交接参数、EL1/MMU/cache 状态、装载字节、BSS/栈清零及保留程序头。EL0 测试检查原地物理映射、运行库设置栈、BootInfo、权限和故障隔离；内存与任务测试继续检查回收、耗尽回滚、所有权、定时器抢占、等待与唤醒。覆盖 debug/release、LOG=off/info 及两种固件入口，日志测试仅在 loader 交接标记之后检查内核输出。
