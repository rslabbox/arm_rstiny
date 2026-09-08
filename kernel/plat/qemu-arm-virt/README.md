# QEMU virt 设备树配置

本目录只维护 overlay 源码。`overlay.dts` 延续 seL4 的 chosen 属性格式，分别选择内核的 UART/GIC/timer 与 elfloader 的 UART/PSCI/timer。

执行 `make platform` 生成 `target/platform/qemu-arm-virt/`：

| 产物 | 内容 |
| --- | --- |
| `qemu-arm-virt.dtb` | QEMU dumpdtb 原始输出 |
| `qemu-arm-virt.dts` | 原始设备树的可读文本 |
| `kernel.dts` / `kernel.dtb` | 合并 overlay 后的最终设备树 |
| `platform.rs` | 内核 MMIO、RAM、timer INTID、PSCI 常量 |
| `devices_gen.h` | loader 的设备和 CPU 描述 |
| `platform_info.h` | loader 可用 RAM 描述 |
| `platform.json` | 供查看的参数和设备选择记录 |
| `platform.sha256` | 工具版本、生成脚本、overlay 和配置的缓存指纹 |

固定平台为 Cortex-A72、单核、128 MiB、GICv3、virtualization=off，从 EL1 启动并使用 PSCI HVC。生成器检查 MMIO、RAM 和中断格式符合当前页表/链接脚本支持范围；它不是任意 ARM 板卡发现框架。修改设备选择或 QEMU 参数时须同时调整平台支持与测试。导出 DTB 和运行 QEMU 必须使用一致的平台参数。

`make build` 先生成平台信息，再编译一套内核和 loader。Cargo 通过 `PLATFORM_DIR` 读取生成配置；不指定该变量的直接 Cargo 构建会在 OUT_DIR 下生成同一固定平台的配置。

`tools/build_image.py` 将对应 kernel ELF、最终 kernel.dtb 和 rootserver 打包为 CPIO，编译原版 seL4 elfloader 时读取生成的 C 头文件。loader 运行时只从归档读取 DTB，无需读取宿主文件或解析 DTS。内核使用编译期常量，不遍历 DTB；DTB 作为扩展 BootInfo 传给 fatboot，由用户态解析。

需要 QEMU、Python 3、device-tree-compiler（dtc/fdtget）。overlay 通过 dtc 合并额外的根节点定义，与 seL4 的 DTS 列表方式相同；它不是需要 fdtoverlay 运行时应用的 /plugin/ DTBO。
