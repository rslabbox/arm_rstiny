# fatboot 启动与用户态边界

当前已实现独立 ELF 的最小 root task，对应设计路线中的“首个用户任务”。参考 `../seL4/projects/sel4test/apps/CMakeLists.txt` 中的 `DeclareRootserver(fatboot)` 和 `apps/boot/main.c` 接收 BootInfo 的启动方式；本项目采用 Rust 和自己的 ABI，不兼容 seL4 二进制，也尚未实现其 CSpace、Untyped 和 IPC 语义。

## 构建与启动

1. `projects/apps/fatboot` 独立链接为静态 AArch64 ELF64 ET_EXEC，入口 `0x400000`。运行库的 `#[entry]` 生成 `_start`，校验 BootInfo 并调用普通 Rust `main`；初始栈由内核提供，不需要单独的汇编 CRT。
2. `tools/pack_root.py` 检查架构、ELF 头、段范围、对齐、权限和入口，拒绝动态装载、解释器、TLS、共享装载页与 RWX 段。模块包含 magic、入口、段数、段描述和初始化字节，不存储 BSS 的零填充内容。
3. `kernel/build.rs` 将 `ROOT_IMAGE` 复制到构建输出，`root_task.rs` 嵌入并再次校验模块。内核从启动堆分配清零页，复制段内容，建立栈、BootInfo 和 IPC buffer。
4. `arch/user.rs` 建立用户页表，维护新代码的 D-cache/I-cache 一致性，切换 TTBR0 并失效 TLB。初始 x0 是 BootInfo 地址，其余通用寄存器为零，SP_EL0 按 16 字节对齐，SPSR 选择 EL0t 并屏蔽中断。
5. 最小寄存器恢复汇编执行 `eret`。fatboot 校验 BootInfo、执行 SVC 往返、检查 IPC buffer 初值并写入结果，然后暂停自身。

执行 `make run LOG=info` 可看到：

```text
[fatboot] root task started in EL0; BootInfo accepted
[fatboot] SVC round-trip passed; suspending
```

此后内核等待于 `root_idle`，由 Ctrl+C 退出 QEMU。`LOG=off` 时同样执行用户程序，仅关闭输出；不能把没有串口输出当成没有启动。

## 地址空间

| 用户虚拟范围 | 用途与权限 |
| --- | --- |
| `0x400000..0x500000` | ELF 装载窗口；实际仅映射装载段，text RX、rodata R/NX、data/BSS RW/NX |
| `0x5f8000..0x5f9000` | BootInfo，用户只读、不可执行 |
| `0x5f9000..0x5fa000` | 专用 IPC buffer，清零、RW/NX；暂未实现 IPC |
| `0x5fb000..0x5fc000` | 栈下保护页，不映射 |
| `0x5fc000..0x600000` | 16 KiB 用户栈，RW/NX |

用户空间采用独立 TTBR0 根，其内核映射保持低地址恒等映射和 supervisor-only 权限，TTBR1 继续禁用。这是当前相对原设计高地址 TTBR1 方案的调整；EL0 隔离由实际页权限保证，并有非法访问测试验证。后续多地址空间设计可以再迁移高地址内核。

用户页均禁止 EL1 执行；内核通过仅 EL1 可访问的启动堆别名初始化它们。该别名不会映射给 EL0。当前仅启动一个任务，页表为静态对象，用户页面从有界堆分配；尚无动态 map/unmap、能力授权、页回收或 ASID 管理。

## Rust 用户接口分层

参考 [seL4 Rust 的 root task 教程](https://docs.sel4.systems/projects/rust/tutorial/root-task/hello-world.html)，其 `sel4` crate 提供 API，`sel4-root-task` 提供运行环境和 `#[root_task]` 入口。另一个参考是 [Hubris userlib](https://github.com/oxidecomputer/hubris/blob/master/sys/userlib/src/lib.rs)，它将带 Rust 类型的系统调用接口与底层寄存器 stub 放在用户库中。

本项目按职责拆分，不复制这些项目的 syscall ABI：

| 位置 / crate | 职责 |
| --- | --- |
| `projects/libs/abi` / `kernel-abi` | 内核和用户库共享的二进制结构、调用号和常量 |
| `projects/libs/user` / `rstiny` | 私有 SVC stub、类型化错误、`yield_now`、`suspend_self`、`debug_putchar`、格式化 `debug_println!` |
| `projects/libs/runtime` / `rstiny-runtime` | `#[entry]`、BootInfo 验证、安全启动信息视图、默认用户 panic handler |
| `projects/apps/fatboot` | 普通 Rust `main` 和应用启动逻辑，不直接依赖 ABI crate |

应用入口写成：

```rust
#![no_std]
#![no_main]
use rstiny_runtime::{entry, BootInfo};

#[entry]
fn main(info: &mut BootInfo) -> ! {
    rstiny::debug_println!("debug console: {}", info.debug_console_available());
    rstiny::suspend_self()
}
```

`rstiny-runtime` 重导出 `rstiny-runtime-macros` 中的 `#[entry]` 属性宏，应用无需直接依赖宏 crate。宏生成固定入口，拒绝 async、unsafe、泛型、外部 ABI 和错误参数/返回值形式，具体 BootInfo 类型由编译器检查。入口要求不返回，应用显式暂停自身。

运行库只构造一次 BootInfo 视图。IPC buffer 通过 `&mut BootInfo` 借用，不向应用暴露可任意构造的指针。当前内核不会异步读写该 buffer；未来 IPC 接口必须延续独占借用约束。默认用户 panic 通过 debug 接口尽力打印原因并暂停，不影响内核 panic 无条件打印及关机策略。

fatboot 中保留应用启动检查和结果符号，供静默启动验证使用；未知 syscall 的验证由宿主注入测试完成，不向应用公开任意调用号接口。

## ABI

共享定义位于 `projects/libs/abi/src/lib.rs`。BootInfo 使用 `repr(C)` 的固定宽度字段：magic、version、size、page_size、features、ipc_buffer、image_start、image_end、stack_start、stack_end。当前版本为 1；features 的 bit 0 表示临时 debug 字符接口可用。不发布尚未实现的 capability 槽。

系统调用执行 `svc #0`，x8 为调用号，x0 为参数和返回值；其他通用寄存器以及用户 SP 保留。返回码：0 成功、1 不支持、2 参数错误。

| x8 | 调用 | 当前行为 |
| --- | --- | --- |
| 0 | Yield | 只有一个可运行任务，直接返回成功 |
| 1 | DebugPutChar | x0 为 0..255 的字节；超界返回参数错误，LOG=off 返回不支持 |
| 2 | SuspendSelf | 保存上下文并暂停任务，内核 idle，不返回 |
| 其他 | 未知调用 | 返回不支持 |

DebugPutChar 通过内核串口输出，不接受用户指针，也不是串口驱动服务。内核普通日志仍走原有彩色 `log`；此接口只提供最早期用户调试输出。未来用户串口服务通过能力及 IPC 独立于 LOG 工作。

用户不可恢复异常记录到 `LAST_FAULT`，任务转为 Faulted，内核 idle；内核自身致命异常和 panic 则诊断后 PSCI 关机。当前没有恢复任务接口或故障 endpoint。目标使用 softfloat，暂不支持 FP/SIMD 上下文及抢占式调度。

## 验证

`make check` 包括三个脚本：

- `test_pack_root.py`：有效模块内容，以及截断、架构错误、非法段、共享页、非法入口等输入。
- `check_kernel.py`：在 `start_root` 前核对内核初始化，显式跳转关机入口验证 PSCI；保留日志、页表、分配器和内核故障/panic 回归。
- `check_fatboot.py`：QEMU GDB stub 验证真实 EL0t、初始寄存器、BootInfo、全部用户映射、SVC 的寄存器保存、应用结果和暂停状态；篡改 BootInfo 版本验证运行库拒绝进入应用，并通过默认 panic 路径暂停。通过实际 EL0 指令验证读内核/UART、写 text/BootInfo、读保护页/空地址、执行栈均停止用户任务，内核继续存活。

用户集成覆盖 debug/release × LOG=off/info，正常启动分别测试 EL1 与 EL2 固件入口。测试只依赖 Python 标准库及 QEMU，不要求安装 GDB 客户端。现在已经能启动最小 fatboot；读盘、FAT32、加载其他程序需要后续能力系统、线程和 IPC 支持。
