# fatboot 启动与用户态边界

当前已实现独立 ELF 的最小 root task，对应设计路线中的“首个用户任务”。参考 `../seL4/projects/sel4test/apps/CMakeLists.txt` 中的 `DeclareRootserver(fatboot)` 和 `apps/boot/main.c` 接收 BootInfo 的启动方式；本项目采用 Rust 和自己的 ABI，不兼容 seL4 二进制，也尚未实现其 CSpace、Untyped 和 IPC 语义。

## 构建与启动

1. kernel 和 fatboot 分别链接成静态 ELF。fatboot 链接脚本将 16 KiB 栈放进 RW 的 NOLOAD PT_LOAD；`#[entry]` 生成设置 SP 的裸入口及 Rust 启动代码。
2. `tools/elf_image.py` 校验 ELF 架构、段、权限、入口和平台装载范围；`tools/build_image.py` 将 `kernel.elf`、`kernel.dtb`、`rootserver` 按顺序打包为 newc CPIO，链接进原版 seL4 elfloader。详见 [引导链](boot.md)。
3. elfloader 装载两个 ELF、清零 BSS/栈，保留用户程序头，开启 MMU 并通过 x0..x5 向 EL1 内核交接。内核接管已装载的物理页，根据保留的程序头建立用户 W^X 映射，不再复制用户段。
4. 内核在用户镜像结束处创建 IPC buffer，下一页创建 BootInfo，后续页存放含 DTB 的扩展 BootInfo。首次恢复进入 EL0t，x0 为 BootInfo，其余 GPR 和 SP 为零；IRQ 开启。运行库入口设置 SP 为 `__user_stack_top`，再校验 BootInfo 并调用 Rust main。
5. fatboot 使用 rs-fdtree 解析 DTB，检查 GICv3、UART 和 PSCI，再执行 SVC 往返、检查 IPC buffer 初值并写入结果，然后暂停自身。

执行 `make run LOG=info` 可看到：

```text
[fatboot] root task started in EL0; BootInfo accepted
[fatboot] SVC round-trip passed; suspending
```

此后内核等待于 `root_idle`，由 Ctrl+C 退出 QEMU。`LOG=off` 时同样执行用户程序，关闭内核日志及用户 debug 输出；elfloader 仍输出引导信息。

## 地址空间

| 用户虚拟范围 | 用途与权限 |
| --- | --- |
| `0x400000..0x500000` | ELF 装载窗口；实际仅映射装载段，text RX、rodata R/NX、data/BSS RW/NX |
| `0x601000..0x602000` | BootInfo，用户只读、不可执行 |
| `0x602000` 起，长度按页取整 | 扩展 BootInfo：记录头和 DTB，只读、不可执行，末页余量清零 |
| `0x600000..0x601000` | 专用 IPC buffer，清零、RW/NX；暂未实现 IPC |
| `0x5fb000..0x5fc000` | 栈下保护页，不映射 |
| `0x5fc000..0x600000` | 16 KiB 用户栈，RW/NX |

用户空间采用独立 TTBR0 根，内核与设备映射位于 TTBR1 高地址区，禁止 EL0 访问；用户低地址空间不保留内核恒等映射。BootInfo 的 image_end 为包含 ELF 栈段的 `0x600000`。

用户页均禁止 EL1 执行；内核通过仅 EL1 可访问的物理帧池别名初始化它们。该别名不会映射给 EL0。现在支持私有动态页表、map/unmap/protect、页面回收和多任务轮转；仍未实现完整 capability 或 ASID 管理。具体边界见 [用户内存与单核任务调度](memory-task.md)。

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

共享定义位于 `projects/libs/abi/src/lib.rs`。BootInfo 使用 `repr(C)` 的固定宽度字段：magic、version、size、page_size、features、ipc_buffer、image_start、image_end、stack_start、stack_end、extra、extra_size。当前版本为 2，大小为 96 字节；features 的 bit 0 表示临时 debug 字符接口可用。不发布尚未实现的 capability 槽。

扩展区的 FDT 记录包含 u64 id=6 和 u64 len，随后为完整 DTB；len 包含 16 字节记录头。映射被固定以保证运行库只读切片有效，应用通过 `info.device_tree()` 获取 DTB。内核不解析设备节点，fatboot 依赖 `rs_fdtree` 完成解析。

系统调用执行 `svc #0`，x8 为调用号，x0 为参数和返回值；原有调用保留其他通用寄存器以及用户 SP；新增带返回值的调用用 x1 返回结果。完整调用号及错误码见 [内存与任务 ABI](memory-task.md)。

| x8 | 调用 | 当前行为 |
| --- | --- | --- |
| 0 | Yield | 让出 CPU，其他 Ready 任务可运行，之后返回成功 |
| 1 | DebugPutChar | x0 为 0..255 的字节；超界返回参数错误，LOG=off 返回不支持 |
| 2 | SuspendSelf | 保存上下文并暂停任务，调度其他 Ready 任务；无就绪任务时 idle |
| 3..19 | 内存与任务操作 | 见内存与任务 ABI |
| 其他 | 未知调用 | 返回不支持 |

DebugPutChar 通过内核串口输出，不接受用户指针，也不是串口驱动服务。内核普通日志仍走原有彩色 `log`；此接口只提供最早期用户调试输出。未来用户串口服务通过能力及 IPC 独立于 LOG 工作。

用户不可恢复异常记录到 `LAST_FAULT`，任务转为 Faulted，释放其地址空间并调度其他任务；内核自身致命异常和 panic 则诊断后 PSCI 关机。现在有暂停/恢复、等待、退出/销毁接口和抢占调度；故障任务只保留终止结果，尚无故障 endpoint 或恢复故障任务的接口。目标使用 softfloat，FP/SIMD 显式陷入内核故障处理。

## 验证

`make check` 包括宏单元测试，以及以下脚本：

- `test_elf_image.py`：有效 ELF 元数据和装载布局，以及截断、架构错误、非法段、共享页、非法入口等输入。
- `check_kernel.py`：在内核入口核对 loader 六个参数、原始段字节、BSS 清零及保留程序头，再在 `start_root` 前核对内核初始化，显式跳转关机入口验证 PSCI；保留日志、页表、分配器和内核故障/panic 回归。
- `check_fatboot.py`：QEMU GDB stub 验证真实 EL0t、初始寄存器、BootInfo、全部用户映射、SVC 的寄存器保存、应用结果和暂停状态；验证 DTB 与归档原文一致，篡改 BootInfo 版本或扩展头验证运行库拒绝进入应用，并通过默认 panic 路径暂停。通过实际 EL0 指令验证读内核/UART、写 text/BootInfo/DTB、读保护页/空地址、执行栈均停止用户任务，内核继续存活。

`check_tasks.py` 另行验证动态内存和多任务抢占，详见内存与任务文档。

用户集成覆盖 debug/release × LOG=off/info，固定使用 EL1 固件入口。测试脚本依赖 Python 标准库及 QEMU；构建另需 dtc/fdtget 和交叉工具链，不要求安装 GDB 客户端。现在已经能启动最小 fatboot；读盘、FAT32、加载其他程序需要后续能力系统、线程和 IPC 支持。
