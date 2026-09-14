# 图形显示栈设计（QEMU virt + virtio-gpu）

在 RSTiny 上加一块"画布"：一个受信的 `gpu-server` 服务驱动 virtio-gpu，
把帧缓冲以**能力租约**方式交给一个 GUI 客户端，软光栅画矩形/位图字/文本。
不追求 3D、合成与矢量字体——目标是"微内核上最基本可用的图形栈"，与
[disk-driver.md](disk-drriver.md) 的 D 阶段同一风格、同一信任边界写法。

## 1. 目标与非目标

| 目标 | 验收 |
| --- | --- |
| `gpu-server` 初始化 virtio-gpu，取得显示参数（分辨率/帧缓冲字节） | D1 自检 |
| 一个 GUI 客户端拿到帧缓冲能力，画色条/矩形/文本并 FLUSH | D2 screendump + 校验和 |
| mysh `./gui` 启动演示应用（复用 `./<program>` + 能力授予） | D2 |
| (可选) virtio-keyboard 输入 → 客户端 | D4 |

非目标（明说）：
- 不做 3D/GPU 加速、窗口合成、双客户端（首版**单客户端全屏租约**）；
- 不做矢量字体/TrueType，用内置位图字库；
- 不做像素格式协商（固定 32bpp）。

## 2. 硬件/平台事实（已核实）

- QEMU `virt` 支持 `-device virtio-gpu-device`（mmio 总线，还有
  virtio-keyboard/mouse）。
- 整块 VirtIO MMIO 窗已是**一个设备 Untyped**（`build_platform.py` 把 32 个
  0x200 槽合成整窗发布，`VIRTIO_MMIO_BASE/SIZE/SIZE_LOG2`）；**加 GPU 不需要
  新设备发布**，驱动像 block-server 一样用 `MmioTransport` 探测
  `DeviceType::Gpu` 选自己的槽。
- 每个 MMIO 槽的中断已按槽位进 `IRQ_LINES`（[irq.md](irq.md) §3.1），gpu 的
  中断可走现有 IRQControl→IRQHandler→Notification 授权链。
- 驱动依赖：`virtio-drivers 0.13` 已带 `VirtIOGpu` 与 `VirtIOInput`
  （`device/gpu.rs`、`device/input.rs`），与 block-server 同 crate。
- 显示后端：CI 无显示器 → 验收主通道用 **guest 侧校验和**（画完对帧缓冲
  算校验和打日志，比块/fs 的 checksum 同款）；宿主机截图用
  `-display egl-headless` + monitor `screendump`（需 GL，做"肉眼演示"用，
  不进 CI 断言）。

## 3. 架构与信任边界

```
                    command/lease           frame cap
  GUI client  <----------------------  gpu-server  <-----> virtio-gpu
     (app)            gpu_ep              (trusted)       (DMA)
                                      framebuffer (own budget)
                                     IRQ → Notification
```

- **gpu-server**：第 6 个服务（console/block/fs/mysh/gpu），`device =
  virtio-mmio-*` 整窗受信组件（无 IOMMU，DMA 边界同 block-server 写法，
  [microkernel-design.md](microkernel-design.md) 已有结论）。
- 初始化：`VirtIOGpu::new` → `GET_DISPLAY_INFO`（分辨率）→
  `RESOURCE_CREATE_2D` + `SET_SCANOUT` → `RESOURCE_ATTACH_BACKING`/
  `TRANSFER_TO_HOST`/`FLUSH`；帧缓冲从**服务自身 budget** retype 的帧构成
  （预算：640×480×4 ≈ 1.2 MiB，`budget = 4M` 起步，含 DMA 页与二次缓冲）。
- **单客户端全屏租约**：`LEASE` 时把帧缓冲的 Frame/VM 能力经 `reply_cap`
  **转给**客户端（能力唯一权威；只给一帧，转交后服务不留别名，或保留只读
  副本由服务校验——首版直接交出所有权，`RELEASE` 收回）。
- 客户端写帧缓冲 → `FLUSH(rect)` → 服务 `RESOURCE_FLUSH`。无合成、无窗口。
- **GUI 库**（Rust，`libs/gui` 或并入协议侧）：位图字库 + `fill_rect`/
  `h_line`/`put_text`/`scroll_up` 软实现，直接写租到的 buffer；只在
  客户端地址空间跑。

## 4. 服务协议（`gpu_ep`，新段，沿用 label 分段约定）

| label | 方向 | 说明 |
| --- | --- | --- |
| `BIND` | client→server | mr0=版本；回复版本 + 分辨率（宽/高/字长） |
| `LEASE` | client→server | server 以 `reply_cap` 转交帧缓冲 Frame/VM 能力；回复显示参数 |
| `FLUSH` | client→server | mr0=x, mr1=y, mr2=w, mr3=h（脏矩形）；server 执行 RESOURCE_FLUSH |
| `RELEASE` | client→server | 收回租约（server 重建帧缓冲或复用） |
| `INPUT_READ`（D4） | client→server | 非阻塞读一个输入事件（key/mouse） |

## 5. 与现有机制的关系

- 设备 Untyped：无需新发布（整窗已给）；gpu 槽位与 IRQ 位置由 QEMU 设备序
  决定，driver 靠 DeviceType 探测（同 block）。
- IRQ：复用授权链（[irq.md](irq.md)）；gpu 完成队列中断 →
  Notification → 服务唤醒。
- 预算：帧缓冲 1.2–3 MiB → `gpu-server budget = 4M`（对齐 fs v2 后 4M 先例）；
  客户端不另切大 untyped。
- fs v2（可选）：位图字库/图片放磁盘（`fonts/`、`.bin`/`.rgba`），
  `./gui` 从 fs 读——演示"长名 + 能力"组合拳。
- MicroPython（stretch）：给 port 暴露**`framebuf` 等价模块**（写租约缓冲），
  Python 画图；依赖 P2 的 fs v2 已实现的 IPC buffer 大消息。

## 6. 分阶段（对齐 D 阶段风格）

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| D0 平台 | QEMU 加 `virtio-gpu-device`（+D4 的 keyboard）；确认 MMIO/IRQ 无需新发布；`check_gpu_platform` 探到 Gpu 设备 | guest 打印 device type/分辨率 |
| D1 驱动 | `gpu-server`：VirtIOGpu 初始化 + 帧缓冲 + 纯色 FLUSH；自检填色 + 校验和 | `[gpu] test flush sum=…`；可选 host screendump 肉眼 |
| D2 租约 | `LEASE/FLUSH/RELEASE` 协议；mysh `./gui`（演示 app：色条+文本）经能力授予跑 | `check_gpu.py`：`./gui` 画完校验和断言 + 正常退出 |
| D3 库 | `libs/gui` 软光栅 + 位图字库；滚屏/简单菜单 | `./gui text/scroll` 校验和用例 |
| D4 输入 | virtio-keyboard → Notification → 服务 → `INPUT_READ` | 喂键序（QEMU `sendkey`/ps）断言客户端收到 |
| D5 stretch | MicroPython `framebuf`、双客户端合成 | 远期，不排期 |

## 7. 风险与不做项

- CI 无显示器：把"截图"当可选，**校验和是主验收**（已有 fs/block 先例）；
  `-display egl-headless` 依赖 GL，只用于人工演示。
- 帧缓冲按像素全屏写，无合成：单客户端模型下够用；双客户端/窗口留 D5。
- virtio-gpu GL 与 2D 差异：`virtio-gpu-device`（非 -gl）是纯 2D 控制队列，
  与 `VirtIOGpu` 匹配；不引入 -gl。
- 不做：3D、加速、字体渲染引擎、鼠标光标合成、多显示器。

## 8. 与 seL4/其他微内核对照

- seL4 生态的图形栈走 **Linux guest**（weston/wayland）或 CAmkES 组件化
  driver + 自定义合成；没有"官方 seL4 裸图形栈"。本项目走**原生受信服务 +
  单客户端租约**，是自有路径（偏离已登记到 [interpreter-app.md](interpreter-app.md)
  的"原生 vs guest"同一段）；QNX/Genode 则有原生图形框架（photond/aarch64、
  Nitpicker）——本项目不引入那么重的合成框架。

## 9. 联动

- 平台/设备：[disk-driver.md](disk-driver.md)（VirtIO/设备 Untyped/DMA 信任边界）
  · [irq.md](irq.md)（中断授权）；
- 服务模型：[service-manager.md](service-manager.md)（第 6 服务、budget、
  READY/EXIT/fault）；
- 依赖栈：[micropython-port.md](micropython-port.md)（framebuf stretch）、
  [roadmap-next.md](roadmap-next.md)（先决：P0 长消息正确性已就绪）；
- 能力模型：[capability-authority-untyped.md](capability-authority-untyped.md)
  （帧缓冲能力租约是该模型的又一次落地）。

（规划文档，实施顺序与是否开工由后续 PR 决定。）