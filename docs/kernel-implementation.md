# 内核实现与验证记录

日期：2026-09-08。

本次完成“内核底座 + 可关闭的 debug 串口”。独立用户程序、BootInfo、能力系统、GIC/定时器调度将在后续阶段实现。

## 完成内容

| 领域 | 实现 |
| --- | --- |
| 项目结构 | 根 Cargo workspace 接入 `kernel/`，内核专用 build.rs 传递链接脚本，保留 `projects/`、`xtask/` 供后续扩展 |
| 构建 | 统一使用 `kernel` 包名与产物名；按优化级别/LOG/test 配置分目录；默认无磁盘无网络 |
| 启动 | 汇编明确区分 EL2 → EL1 与直接 EL1 入口；主核独占初始化；建立栈、早期异常向量、清零 BSS |
| 内存 | 4 KiB 对齐的四级页表；从首次启用 MMU 起就是细粒度映射；text RX、rodata R/NX、data/BSS/heap/stack RW/NX |
| 保护 | 所有内核页禁止 EL0 访问；启用 WXN；栈下留未映射页；禁用 TTBR1，去掉高地址别名和大范围设备映射 |
| 分配 | 保留有界 16 MiB 启动堆及原分配器测试；检查范围位于约定 RAM 内；不对外分配未映射 RAM |
| 日志 | 默认依赖 log，标准日志宏统一走 LOG 级别；LOG=off 时关闭普通输出，不求值被过滤的日志参数 |
| UART | 参考 arm_pl011，以 tock-registers 定义寄存器结构及位字段；独立 Pl011Uart 驱动提供初始化和轮询 TX，console 处理 UTF-8 与换行；无设备 IRQ，轮询有界 |
| 故障 | 每个向量入口记录 kind/source；ESR/FAR/寄存器先保存后打印；异常与 panic 最终屏蔽中断并停机 |
| 自测 | kernel-test 单独开启；分配器失败有断言；故障探针仅在测试构建保留 |

默认路径不执行旧的 `user_main()`；原先这个 EL1 普通函数已移除，避免误认成用户态。现有 `utils/timer.rs` 尚未接入运行路径，读计数器不等于已实现 timer IRQ。

## 实际验证

通过 `make check`（底层是 `tools/check_kernel.py`）完成以下检查。脚本只依赖 Python 标准库与 QEMU GDB stub，通过本地 Unix socket 读取 CPU/内存；每个故障用例启动独立 QEMU，并设连接、执行超时和进程清理。

- dev/release × LOG=info/off，共四种正式构建，全部到达 `kernel_idle` 且 `BOOT_STATE=3`。
- 四种正式构建分别验证 EL1 与 EL2 入口，最终 CPU 为 EL1h，异常屏蔽位正确。
- 遍历实际页表，核对全部映射集合、恒等物理地址、页面大小、AP、PXN/UXN、内存属性、保护页及始终保留的 UART 页。
- 核对 MMU/cache/WXN、TTBR1 禁用和 VBAR/栈对齐。
- 四种对应测试构建运行分配器自测，检查成功状态；LOG=off 的测试验证日志参数不会求值。
- 注入 BRK，检查 ESR 中的异常类及立即数。
- 写代码页触发页权限错误；执行栈触发指令权限错误。
- 读取栈保护页、空地址触发翻译错误。
- 注入 panic，验证与异常不同的状态值和共同的停机入口。
- LOG=off 的正常启动和普通异常日志保持静默；panic 在所有等级直接打印。另注入 logger 锁已占用时的 panic，验证输出不被锁阻塞。
- 全部六个日志级别均构建运行；error/warn 过滤 info，error 仍输出 panic 诊断。
- 正式 ELF 不包含 `probe_*`；LOG=off 验证正常启动静默，不要求 logger 从 ELF 消除。

另外通过 `cargo fmt --all --check` 与 Clippy 检查。裸机 binary 禁用了标准宿主 test harness，集成验证使用 `make check`，不是在 AArch64 bare-metal target 上运行标准 `cargo test`。

## 当前边界与下一步

当前固定平台为 128 MiB QEMU RAM，链接脚本限制整个内核及堆落在最初 32 MiB 页表覆盖区内。没有解析 DTB、分配剩余 RAM、实现动态 map/unmap 或地址空间切换。链接脚本预留栈和堆为 NOLOAD，避免把 16 MiB 零字节塞进启动 bin。

异常记录面向仍有有效内核栈的故障。保护页能阻止越界访问，但真正耗尽异常保存栈后的双重故障尚无专用紧急栈恢复能力。

普通日志使用 try-acquire；panic 屏蔽中断后使用独立串口输出，绕过 logger 锁并以原子标志阻止递归打印。串口不可用时输出仍受轮询上限约束，随后停机，调试器可读取状态。用户态串口服务的所有权交接尚未出现，因为目前没有用户态串口服务。

下一步建立独立用户 ELF/CRT、初始用户页表与 TCB、BootInfo 和 SVC 往返，再启动真正的 EL0 fatboot。届时必须替换当前低地址内核恒等布局，不能直接在现有 TTBR0 中加入用户页就宣称完成隔离。
