# Rust bootloader

这是项目自己的 `no_std`、无堆分配 Rust 引导程序。它保留此前 seL4 ARM
elfloader 的镜像相对顺序和六寄存器交接协议，代码不再调用上游 C 实现。

当前平台固定为 QEMU virt、Cortex-A72、单核、128 MiB RAM、GICv3、
`virtualization=off`。QEMU 通过 ELF 入口启动它，入口要求 EL1 且 MMU、
指令缓存和数据缓存关闭。它不实现固件重定位、EL2 降级或任意平台发现。

## 构建

日常使用项目根目录的 `make build` / `make run`。镜像构建工具先生成
`newc` CPIO，按顺序包含 `kernel.elf`、`kernel.dtb`、`rootserver`，再调用：

```sh
BOOT_ARCHIVE_OBJECT=/absolute/path/archive.o \
cargo build -p bootloader --target aarch64-unknown-none-softfloat
```

构建工具用 `rust-objcopy` 将归档转为 AArch64 `archive.o`，`build.rs` 仅传递目标文件和链接脚本给链接器。归档位于独立只读段，入口固定在物理地址
`0x44000000`。生成的 ELF 包含引导代码、归档、页表和独立栈，不需要交叉 C
编译器。入口和系统寄存器操作使用 Rust 内联汇编，没有独立汇编源文件。

bootloader 使用源码中的固定平台常量，不需要 `PLATFORM_DIR`，构建脚本不调用 QEMU 或 dtc。未设置
`BOOT_ARCHIVE_OBJECT` 时允许 workspace check/clippy；实际链接缺少归档则立即报错。
生成启动镜像必须使用镜像构建工具或提供真实归档目标文件。

## 装载和交接

在修改目标内存之前，程序校验 CPIO 边界和文件顺序、两个 ELF64 的结构、
段范围及权限、入口、DTB magic/total size，以及完整的物理地址布局。
不支持动态 ELF、解释器和 TLS。DTB 的设备节点在构建时解析，运行时作为不透明数据传递。

内核链接在 `0xffff800000000000` 的独立 32 MiB 高地址窗口，物理位置由
`LoadPlan` 在 RAM 中动态选择，不使用 ELF 的 `p_paddr`。分配时排除固件的前
2 MiB 和 loader 完整范围，以 2 MiB 对齐选择足够容纳内核、DTB、rootserver
和保留程序头页的区间。DTB 在内核之后，rootserver 按页对齐，PHDR 页在其末尾。
`make run KERNEL_LOAD_MIN=0x41000000` 可设置空闲空间搜索下界，内核 ELF 无需修改。

`image.rs` 将 `BootImages::parse`、`LoadPlan::new` 和 `LoadPlan::load` 分开，
先完成全部校验，再消费计划执行物理写入；错误区分内核 ELF、root ELF、DTB、
归档和布局。`layout.rs` 使用明确的物理范围和镜像映射类型，无堆分配。

临时 TTBR0 提供恒等映射，TTBR1 提供固定偏移直接映射及独立内核镜像映射，MAIR 的索引 0
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
不符合契约时，在栈建立之后、BSS 清零和串口初始化之前停机。

内核通过 loader 的活动页表查询自身物理起点，再建立正式映射。因此内核物理位置
不占用额外启动寄存器。详见 [动态映射与验证](../docs/boot.md)。

宿主单元测试使用 `cargo test -p bootloader --no-default-features --test images --target <host>`；
默认 `image` 特性只控制是否构建 AArch64 启动二进制，纯解析/规划测试无需启动镜像。
