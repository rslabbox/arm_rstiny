# ARM RSTiny

AArch64 Rust 微内核实验项目。目前提供内核底座与可选 debug 串口，尚未创建用户态任务。

## 构建与运行

需要 Rust nightly（包含 `aarch64-unknown-none-softfloat` target）、`cargo-binutils`/LLVM tools、QEMU AArch64；集成验证另外需要 Python 3。当前验证环境为 rustc `1.100.0-nightly (5a2be9f5f 2026-09-06)`。

在项目根目录执行：

```sh
# 带启动日志，初始化完成后自动关闭 QEMU。
make run-kernel LOG=info

# 关闭普通日志，正常启动后静默关机；panic 仍打印。
make run-kernel LOG=off

# 优化与日志开关彼此独立。
make run-kernel MODE=release LOG=info
make build MODE=release

# QEMU 集成验证，不要求安装 GDB 客户端。
make check

# 格式与静态检查。
make fmt
cargo clippy --all-targets --all-features -- -D warnings
```

默认运行平台固定为 QEMU `virt,gic-version=2,virtualization=on`、Cortex-A72、单核、128 MiB RAM，无磁盘、无网卡。当前使用低地址恒等映射，TTBR1 禁用；只映射内核、启动栈与固定 16 MiB 堆，始终额外映射一个 PL011 页供 panic 使用。未映射的其余 RAM 尚不提供分配。

输出镜像位于 `target/kernel/<MODE>-log<LOG>-test<0|1>/aarch64-unknown-none-softfloat/<MODE>/kernel[.bin]`。不同功能组合使用独立构建目录。根 Cargo workspace 只包含 `kernel/`，后续用户程序可放 `projects/`；内核链接参数只应用于内核 binary。

`LOG` 是唯一的内核日志配置，默认 `info`，支持 `off/error/warn/info/debug/trace`。日志保留原有 ANSI 颜色：error 红、warn 黄、info 绿、debug 青、trace 灰，包含文件名与行号。`log` 为普通依赖，启动、自测、异常使用 `log::info!`、`log::error!` 等标准宏。`LOG=off` 关闭普通日志。panic 绕过 logger 与其锁，直接初始化串口并打印位置和原因；UART 映射因此始终保留；不承诺从二进制移除 logger 代码。日志级别在构建时读取，修改 `LOG` 会触发重新构建，与 dev/release 优化配置独立。直接执行 Cargo 也可使用 `LOG=off cargo build`。

## 调试与状态

```sh
make debug LOG=info
# 另一个终端；需要支持 AArch64 的 GDB：
gdb-multiarch target/kernel/debug-loginfo-test0/aarch64-unknown-none-softfloat/debug/kernel
```

在 GDB 中：

```text
target remote :1234
hbreak kernel_shutdown
continue
x/gx &BOOT_ENTRY_EL_VALUE
```

在 `kernel_shutdown` 断点处检查关机前状态；异常通过 `LAST_FAULT` 核对，panic 通过独立串口诊断核对。`BOOT_ENTRY_EL_VALUE` 记录入口为 EL1 还是 EL2。异常记录 `LAST_FAULT` 包含 kind/source、ESR/FAR、完整通用寄存器、SP_EL0、ELR/SPSR；FAR 仅在对应异常类规定其有效时解释。`kernel_halt` 仅用于不支持的启动条件或固件关机返回后的停驻兜底。

内核完成初始化（启用 kernel-test 时先运行自测）后调用 PSCI SYSTEM_OFF 关机；致命异常及 panic 也在诊断后关机。当前没有调度器或用户程序。固定 QEMU 平台从 EL2 启动时使用 SMC，从 EL1 启动时使用 HVC；未来支持其他固件时需从平台信息获取调用方式。EL3、安全态、多核运行和热启动不在支持范围内。入口要求 MMU/cache 关闭，启动前已有物理 RAM 可用。

`KERNEL_TEST=1` 对应独立 `kernel-test` feature，启用原有分配器自测及调试器调用的故障探针；正式构建没有探针。自测失败会 panic，成功写 `SELF_TEST_PASSED=1`。测试结果不依赖串口或关机系统调用。

## 文档

- [内核实现与验证记录](docs/kernel-implementation.md)
- [完整微内核设计与分阶段路线](docs/microkernel-design.md)
