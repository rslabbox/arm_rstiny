# ARM RSTiny

AArch64 Rust 微内核实验项目。当前可启动独立的 EL0 用户程序 `projects/apps/fatboot`，传递 BootInfo，并支持独立用户地址空间、可回收页管理及单核定时器抢占调度。

## 构建与运行

需要 Rust nightly（包含 `aarch64-unknown-none-softfloat` target）、`cargo-binutils`/LLVM tools、QEMU AArch64、Python 3、GNU cpio、device-tree-compiler（`dtc`/`fdtget`）和 AArch64 GCC 交叉工具链（`aarch64-linux-gnu-gcc`）。当前验证环境为 rustc `1.100.0-nightly (5a2be9f5f 2026-09-06)`。

在项目根目录执行：

```sh
# 启动内核及 fatboot，完成检查后进入 idle；Ctrl+C 退出 QEMU。
make run LOG=info

# 同样启动 fatboot，关闭普通日志与用户 debug 输出；内核 panic 仍打印。
make run LOG=off

# 优化与日志开关彼此独立。
make run MODE=release LOG=info
make build MODE=release

# 宿主打包器测试、内核与 EL0 集成验证；无需 GDB 客户端。
make check

make fmt
cargo clippy --workspace --lib --bins --all-features -- -D warnings
# 库的标准测试 harness 在宿主运行（下面以 x86_64 Linux 为例）。
cargo clippy -p rs_fdtree --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

`run-root`、`run-kernel` 是 `run` 的别名，均启动完整镜像。默认平台为 QEMU `virt,gic-version=3,virtualization=off`、Cortex-A72、单核、128 MiB RAM，无磁盘、无网卡。

GIC 由 `arm-gic-driver` 的 GICv3 接口管理。内核使用 Group 1 的物理定时器中断（PPI 14 / INTID 30）驱动调度；设备中断向用户态驱动的授权投递尚未实现。

fatboot 在用户态解析 DTB，使用从 x-kernel 引入的 `drivers/platform/rs-fdtree`，见 [解析器来源及修改](drivers/platform/rs-fdtree/UPSTREAM.md)。堆、帧池、调度器、GIC 和日志通过内核内部的 `SingleCore` 封装访问：要求 IRQ 已屏蔽，并检查可变借用不重叠，不使用自旋锁。日志遇到重入时跳过输出；panic 保持独立输出。

构建使用 seL4 原版 ARM elfloader：独立 kernel ELF、fatboot ELF 与 DTB → CPIO → 链接到 elfloader。产物位置：

- 内核：`target/kernel/<MODE>-log<LOG>-test<0|1>/aarch64-unknown-none-softfloat/<MODE>/kernel[.bin]`
- 设备树及生成配置：`target/platform/qemu-arm-virt/`；overlay 源码在 `kernel/plat/qemu-arm-virt/overlay.dts`
- 用户 ELF：`target/apps/<MODE>/aarch64-unknown-none-softfloat/<MODE>/fatboot`
- 启动镜像：`target/kernel/<MODE>-log<LOG>-test<0|1>/image/elfloader`。

Cargo workspace 包含 `kernel`、`projects/apps/fatboot`、`projects/libs/abi`、用户 API `projects/libs/user` 和运行库 `projects/libs/runtime`，内核与用户程序分别使用链接脚本。内核可独立 Cargo 构建，不再嵌入用户程序；直接 Cargo 构建生成固定的 EL1 平台配置到 OUT_DIR，因而也需要 QEMU 和 dtc 工具。通过 `make build` 生成完整启动镜像；QEMU 必须启动 elfloader，`kernel.bin` 仅保留为辅助产物。详见 [seL4 elfloader 引导链](docs/boot.md)。

`LOG` 是唯一的内核日志配置，默认 `info`，支持 `off/error/warn/info/debug/trace`，构建时生效。普通内核日志使用标准 `log` 宏，保留 ANSI 颜色和文件名、行号。`LOG=off` 关闭普通日志和临时用户 debug 字符接口。内核 panic 绕过 logger 及其锁，无条件尝试初始化串口并打印位置和原因，随后通过 PSCI 关机。原版 elfloader 的引导输出独立于内核 `LOG`，因此 `LOG=off` 仍能看到 loader 信息。用户程序 panic 则打印用户诊断（若 debug 接口可用），暂停自身。

## 调试与状态

```sh
make debug LOG=info
# 另一个终端：
gdb-multiarch target/kernel/debug-loginfo-test0/aarch64-unknown-none-softfloat/debug/kernel
```

在 GDB 中：

```text
target remote :1234
hbreak start_root
continue
x/gx &BOOT_ENTRY_EL_VALUE
# fatboot 的固定用户入口；此处可检查 EL0t、SP 和 x0。
hbreak *0x400000
continue
info registers cpsr sp x0
```

`BOOT_ENTRY_EL_VALUE` 记录内核入口特权级，当前平台固定从 EL1 启动。`SCHEDULER` 保存任务表、当前任务和就绪队列，首个任务槽为 fatboot；`LAST_FAULT` 保存最近异常的来源、ESR/FAR 和通用寄存器。EL0 下调试器读取内核地址可能失败；集成测试用 QEMU 物理内存模式读取内核记录和非当前任务内存。

用户页表使用独立 TTBR0；TTBR1 提供仅 EL1 可访问的高地址内核映射。用户不能访问内核、UART 或 GIC。内核元数据堆为 16 MiB；用户页和私有页表从独立 8 MiB 帧池及接管的 2 MiB root image 区间分配，释放后可复用。最多 32 个任务，每个地址空间最多 1024 个用户页；DTB 经扩展 BootInfo 交给 fatboot，尚未用其发现和分配其余 RAM。

内核初始化后进入 fatboot。EL0 由 10 ms 定时器抢占，支持创建、启动、暂停、恢复、睡眠、等待、退出和销毁任务。没有 Ready 任务时内核进入可被定时器唤醒的 idle。用户故障终止该任务并释放其空间，其他任务继续执行。内核致命异常和 panic 仍诊断后关机：使用构建时从最终 DTB 生成的 SMC/HVC 配置。

当前每个任务有一个地址空间和一个用户线程。完整 capability、IPC、fork/COW、文件映射与 POSIX 尚未实现；不支持 EL3、安全态、SMP、FP/SIMD、热启动。elfloader 要求固件交接时 MMU/cache 关闭且 RAM 已可用；进入内核时 MMU/cache 已开启。详见 [用户内存与单核任务调度](docs/memory-task.md)。

`KERNEL_TEST=1` 对应独立 `kernel-test` feature，启用分配器自测与调试器使用的故障探针。自测成功写 `SELF_TEST_PASSED=1`，失败 panic；正式构建不包含探针。

## 文档

- [seL4 elfloader 引导链](docs/boot.md)
- [用户内存与单核任务调度](docs/memory-task.md)
- [fatboot 启动、ABI 与验证](docs/fatboot.md)
- [内核实现与验证记录](docs/kernel-implementation.md)
- [完整微内核设计与分阶段路线](docs/microkernel-design.md)
