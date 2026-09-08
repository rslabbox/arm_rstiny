# ARM RSTiny

AArch64 Rust 微内核实验项目。当前可启动独立的 EL0 用户程序 `projects/apps/fatboot`，传递 BootInfo，并完成 SVC 往返与访问隔离。

## 构建与运行

需要 Rust nightly（包含 `aarch64-unknown-none-softfloat` target）、`cargo-binutils`/LLVM tools、QEMU AArch64 和 Python 3。当前验证环境为 rustc `1.100.0-nightly (5a2be9f5f 2026-09-06)`。

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
cargo clippy --workspace --all-targets --all-features -- -D warnings
```

`run-root`、`run-kernel` 是 `run` 的别名，均启动完整镜像。默认平台为 QEMU `virt,gic-version=2,virtualization=on`、Cortex-A72、单核、128 MiB RAM，无磁盘、无网卡。

构建按“fatboot ELF → 校验并打包启动模块 → 嵌入内核 → kernel.bin”执行。产物位置：

- 内核：`target/kernel/<MODE>-log<LOG>-test<0|1>/aarch64-unknown-none-softfloat/<MODE>/kernel[.bin]`
- 用户 ELF：`target/apps/<MODE>/aarch64-unknown-none-softfloat/<MODE>/fatboot`
- 启动模块：`target/apps/<MODE>/fatboot.boot`

Cargo workspace 包含 `kernel`、`projects/apps/fatboot`、`projects/libs/abi`、用户 API `projects/libs/user` 和运行库 `projects/libs/runtime`，内核与用户程序分别使用链接脚本。完整启动镜像应通过 `make build` 构建；直接 Cargo 检查无需模块，但没有 `ROOT_IMAGE` 的内核构建会在启动 root task 时报告缺少镜像。

`LOG` 是唯一的内核日志配置，默认 `info`，支持 `off/error/warn/info/debug/trace`，构建时生效。普通内核日志使用标准 `log` 宏，保留 ANSI 颜色和文件名、行号。`LOG=off` 关闭普通日志和临时用户 debug 字符接口。内核 panic 绕过 logger 及其锁，无条件尝试初始化串口并打印位置和原因，随后通过 PSCI 关机。用户程序 panic 则打印用户诊断（若 debug 接口可用），暂停自身。

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

`BOOT_ENTRY_EL_VALUE` 记录启动入口为 EL1 还是 EL2。`ROOT_TASK` 保存初始任务状态、地址空间根和上下文；状态依次编码为 Inactive=0、Running=1、Suspended=2、Faulted=3。`LAST_FAULT` 保存异常来源、ESR/FAR 和通用寄存器；FAR 只在架构规定有效的异常类中解释。EL0 权限下调试器读取内核地址可能失败，集成测试使用 QEMU 物理内存调试模式读取这些内核记录。

用户页表具有独立根，保留仅 EL1 可访问的低地址内核映射；TTBR1 仍禁用。用户代码 RX、数据与栈 RW/NX、BootInfo 只读，栈下留保护页。用户不能访问内核或 UART。任务初始化从有界的 16 MiB 内核启动堆分配页面，其余 RAM 尚不对外分配。

内核初始化后进入 fatboot；fatboot 暂停或故障后内核 idle。目前没有多线程调度、能力系统或 IPC 服务。内核致命异常和 panic 仍诊断后关机：固定 QEMU 平台 EL2 入口使用 SMC，EL1 入口使用 HVC。不支持 EL3、安全态、SMP、热启动；入口要求 MMU/cache 关闭且 RAM 已可用。

`KERNEL_TEST=1` 对应独立 `kernel-test` feature，启用分配器自测与调试器使用的故障探针。自测成功写 `SELF_TEST_PASSED=1`，失败 panic；正式构建不包含探针。

## 文档

- [fatboot 启动、ABI 与验证](docs/fatboot.md)
- [内核实现与验证记录](docs/kernel-implementation.md)
- [完整微内核设计与分阶段路线](docs/microkernel-design.md)
