# Rust bootloader

这是项目自己的 `no_std`、无堆分配 Rust 引导程序。它保留此前 seL4 ARM
elfloader 的镜像布局和六寄存器交接协议，代码不再调用上游 C 实现。

当前平台固定为 QEMU virt、Cortex-A72、单核、128 MiB RAM、GICv3、
`virtualization=off`。QEMU 通过 ELF 入口启动它，入口要求 EL1 且 MMU、
指令缓存和数据缓存关闭。它不实现固件重定位、EL2 降级或任意平台发现。

## 构建

日常使用项目根目录的 `make build` / `make run`。镜像构建工具先生成
`newc` CPIO，按顺序包含 `kernel.elf`、`kernel.dtb`、`rootserver`，再调用：

```sh
BOOT_ARCHIVE=/absolute/path/archive.cpio \
PLATFORM_DIR=/absolute/path/platform \
cargo build -p bootloader --target aarch64-unknown-none-softfloat
```

`build.rs` 将归档通过 `.incbin` 放入独立只读链接段，入口固定在物理地址
`0x44000000`。生成的 ELF 包含引导代码、归档、页表和独立栈，不需要交叉 C
编译器。入口和系统寄存器操作使用 Rust 内联汇编，没有独立汇编源文件。

未设置 `PLATFORM_DIR` 时，构建脚本调用项目的平台生成器。未设置
`BOOT_ARCHIVE` 时允许 workspace check/clippy，生成的空归档镜像在启动时
明确报错停机；要生成可启动镜像必须使用镜像构建工具或提供真实归档。

## 装载和交接

在修改目标内存之前，程序校验 CPIO 边界和文件顺序、两个 ELF64 的结构、
段范围及权限、入口、DTB magic/total size，以及完整的物理地址布局。
不支持动态 ELF、解释器和 TLS。DTB 的设备节点在构建时解析，运行时作为不透明数据传递。

内核虚拟地址从 `0xffff000040200000` 开始，物理地址从 `0x40200000`
开始。DTB 紧随内核物理区域，rootserver 从下一个 4 KiB 边界开始；其后
保留一页，存放两个 little-endian `u32`（PHDR 数量和大小）及原始 PHDR。
所有目标内存必须位于 `0x40200000..0x42000000`，与 loader、归档和栈隔离。
ELF 的 BSS、段间空洞和 rootserver 栈在装载时清零。

临时 TTBR0/TTBR1 映射提供物理地址恒等映射及高地址别名，MAIR 的索引 0
为 Device-nGnRnE，索引 4 为 normal WB。开启 MMU/cache 后进入内核：

| 寄存器 | 内容 |
| --- | --- |
| x0 | rootserver 物理起始地址 |
| x1 | rootserver 物理结束地址，不包含保留的 PHDR 页 |
| x2 | rootserver 物理地址减虚拟地址的偏移 |
| x3 | rootserver 虚拟入口 |
| x4 | DTB 物理地址 |
| x5 | DTB total size |

临时映射是引导期的宽松映射，由内核正式页表取代；用户态权限策略由内核
根据保留的 PHDR 建立。进入内核后 loader 不再参与调度或设备管理。

## 错误和调试

串口诊断独立于内核 `LOG`。输入校验失败输出 `bootloader: error:`，然后
停在 `bootloader_halt`；不会进入内核。归档符号 `__archive_start` 和
`__archive_end` 可用于 GDB 检查及非法镜像回归测试。入口特权级或缓存状态
不符合契约时，在使用栈和串口之前停机。
