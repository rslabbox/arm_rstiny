# 图形显示栈设计（QEMU virt + virtio-gpu）

在 RSTiny 上加一块"画布"：一个受信的 `gpu-server` 服务驱动 virtio-gpu，
把帧缓冲以**能力租约**方式交给一个 GUI 客户端，软光栅画矩形/位图字/文本。
不追求 3D、合成与矢量字体——目标是"微内核上最基本可用的图形栈"，与
[disk-driver.md](disk-driver.md) 的 D 阶段同一风格、同一信任边界写法。

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

## 10. 实施记录（D0-D4 已落地）

六个阶段里 D0-D4 全部实现并通过 `tools/check_gpu.py`（已挂进 `make check`）；
D5（MicroPython framebuf、双客户端）仍不排期。与规划的主要偏离如下，
都已在代码注释里就地说明。

### 10.1 设备与预算（§2、§5 的修正）

- **QEMU 接线**：`-device virtio-gpu-device,xres=640,yres=480 -device
  virtio-keyboard-device` 进了 `QEMU_ARGS`（`GPU_ARGS`），设备序
  blk=0/gpu=1/keyboard=2 对应窗口槽 31/30/29；平台 DTB 仍是 32 个连续槽，
  **无需重新发布**（check_gpu.py D0 断言）。分辨率由 `xres/yres` 固定
  640×480（QEMU 默认 1280×800，不设就是 600 页帧缓冲）。
- **设备 Untyped 必须共享**：整窗只有一个 16 KiB 设备 Untyped，4 KiB 帧
  粒度又装不下 0x200 槽的边界——block- 与 gpu-server 都要映射**同一批
  页**。内核改为：设备区域 retype **总是从区域基址开始**、按物理地址
  找到已存在的 Frame 对象时**共享同一对象**（`new_untyped_frame`），
  跳过水位线检查。配套地，init 的 teardown **绝不 revoke 设备区域**
  （`finalise_untyped` 是区域粒度的，会把别的驱动还在用的帧一起回收）；
  设备副本同预算一样是 supervisor 所有、跨重启复用。
- **预算**：gpu-server `budget = 2M`（规划写"4M 起步"）。实测 init 的
  16 MiB 预算里放不下第二个 4M——4M 对齐 + 64 字节端点正好把 mysh 的
  4M 顶出区域（fs v2 的先例也是 2M）。300 页帧缓冲 + 队列环 + 覆盖表
  ≈ 310 页，2M 够用。init 的基础设施（预算/端点/设备副本/IRQ 副本）
  改为**启动期一次建好**：预算先于端点 retype，否则 64 字节端点卡在
  对齐边界上会把后面每个预算都顶到下一个边界。
- **每服务两个 IRQ**：`SpawnInfo::IRQ_SLOT2 = 10`（槽 9 在 init 自己的
  页面上是 userboot 写的中断线数，服务槽位避开它）；init 按配置的
  `device` 行序授权多条 IRQHandler。

### 10.2 租约协议（§4 的落地形态）

内核一条消息最多带 3 个能力（`MAX_EXTRA_CAPS`），300 帧不可能一次交付，
所以协议变成**分批**（labels 段 0x600）：

| label | 方向 | 说明 |
| --- | --- | --- |
| `BIND` | client→server | 回版本 + 宽/高/帧缓冲字节 |
| `LEASE` | client→server | 交出帧 0..3 的 Frame caps；回总页数/字节 |
| `LEASE_BATCH` | client→server | mr0=首帧，交 3 帧；重复至拿满 300 帧 |
| `FLUSH` | client→server | 脏矩形（校验用）；提交仍是整屏 |
| `RELEASE` | client→server | mr0=首帧 + 归还 3 帧；收满 300 帧租约结束 |
| `INPUT_READ` | client→server | 非阻塞读一个输入事件（打包进一个字） |

- **所有权**：首版直接交出（§3 的第一选项）。服务端**保留 master caps
  但解除自己的映射**——租期内客户端崩溃时服务端还能恢复（若把 cap 也
  删掉，客户端一死帧就无人引用了）。`RELEASE` 归还的每一批都按物理地址
  验明正身（能力是权威、物理地址是身份），然后删掉落槽副本；收满 300 帧
  服务端重新映射、租约释放，同一轮 shell 里第二个 `./gui` 可再次租用。
- **FLUSH 脏矩形 v1 仍整屏提交**：virtio-drivers 0.13 的
  `transfer_to_host_2d(rect)` 是私有 API，`flush()` 只有整屏形态。
- **接收规格**：receive spec 粘在 IPC buffer 里且落槽必须为空，所以
  每一批 LEASE/RELEASE 前都要重写 spec（`LEASE_BATCH`/`RELEASE` 各 100 轮）。
- **槽位布局**：gpu-server 的 DMA 槽窗口从 200 起（300 帧横跨 300 个
  槽，绝不能撞上子进程固定槽 51/52/53——第一次跑就撞了）；RELEASE 落槽
  单独一段（616..），与保留的 master caps 分开。

### 10.3 驱动与验收（§6）

- **帧缓冲分配**：`virtio-drivers` 的 `Dma` 要求**物理连续**；内核一次
  retype 上限 32 个对象，300 页在 Hal 里**分块重类型化**（顺序水位线
  保证连续）。Hal 记录每次分配，帧缓冲就是"恰好 300 页的那次分配"。
- **D1 自检**（`GPU_TEST=1`）：16 行灰度带 + 整屏 flush + 字校验和
  `[gpu] test flush 640x480 pages=300 sum=…`。
- **`libs/gui`**：8x8 位图字库（font8x8 basic 集，公有领域，0x20..=0x5F
  共 64 个字形，LSB 为最左列）+ `fill_rect`/`draw_text`/`scroll_up`/
  `word_sum`。**字库与调色板是验收的单一事实来源**：`check_gpu.py` 直接
  解析 `font.rs` 与两个 `PALETTE`/`BARS` 常量，宿主侧按同一套光栅规则
  重算校验和，不复制第二份表。
- **gui 演示 app**：`./gui bars|text|scroll|keys N`（argv 走决策 H 的
  ArgvBlock；决策 I 的"带参程序拿依赖端点"扩展成 fs + gpu 两个端点，
  mysh 的 `depends = fs gpu`）。`keys` 场景轮询 `INPUT_READ`，按键经
  monitor `sendkey` 注入 virtio-keyboard，打印 `[gui] key: a` 等。
- **check_gpu.py**：D0 平台契约（dumpdtb 复查 32 槽连续）+ D1（两种
  编译模式）+ D2/D3/D4（模式×日志级矩阵，一次 shell 会话里跑完四个
  场景，bars 跑两遍顺带验证 RELEASE 后可再租）。

### 10.4 已知限制

- 客户端在租期内崩溃：租约卡住（服务端留着 master caps 但按 v1 语义
  拒绝新 LEASE），重启 gpu-server 或整轮系统恢复；v2 可以让 init 在
  reap 时发一个"强制收回"。
- INPUT_READ 的键盘事件主要靠轮询 `pop_pending_event`；中断 → Notification
  链路已授权并绑定（与 gpu 完成中断一样在 FLUSH/READ 后 ack），但没有
  实现"事件到达唤醒服务"的阻塞等待——与 console READ 的先例一致。
- 无光标、无双客户端、无矢量字体（§1 非目标维持不变）。
- **已解决（2026-09 晚于 a3d922b）**："六服务启动竞态"的根因不是内核、
  也不是 QEMU 的 IRQ 层，而是 **`MmioTransport` 的 `Drop` 会把设备复位**
  （virtio-drivers 0.13 `impl Drop` → `set_status(empty)`）。三个 probe
  函数对窗口内每个槽位构造 transport、类型不匹配就地 `continue`——drop
  即复位。时序：block-server 先初始化 blk 成功；gpu-server 的
  `probe_inputs` 扫过 mouse/kbd 后继续扫 gpu 与 blk 槽，两个 transport
  被 drop，**把已就绪的 gpu 和 blk 全部复位**；fs 挂载的 READ 提交进一个
  status=0、vring=0 的空设备（QMP `x-query-virtio-status` 实证
  `started=false`、`last_avail_idx=0`），完成中断自然永不触发，mysh→fs→
  block 三级死锁。此前所有"竞态/GIC 路由失稳"的表象都是它的次生噪声
  （复位设备的线被 UNROUTED 路径禁用等）。修复：probe 中类型不匹配的
  transport 用 `core::mem::forget` 保活（它只包裸指针、无堆分配），
  `probe`/`probe_gpu`/`probe_inputs` 三处同改。
- wm 场景遗留（截图验收时发现）：字库原只覆盖 0x20..=0x5F，小写字母
  全部渲染成 `?`——font.rs 已用 fontgen 工程重新生成 0x20..=0x7E
  （95 字形，check_gpu.py 的 `load_font` 断言 ≥64 仍兼容）。另：QEMU
  `sendkey kp_add/kp_enter` 在 wm 键码表里的映射有偏差（注入 1+2= 得到
  "33"），桌面键盘路径未复查，留待 wm v2。
- **已解决（2026-09-16）**：respawn 活锁（check_fault_handler 的阻塞点，
  a3d922b 既有）。根因是**内核对象回收的两个缺口叠加**：
  ①收集器的 mark 把**不可达 CNode 里的 cap** 也当根——loader 给每个子
  CSpace 装了自引用 cap，整个被销毁的进程组（VSpace/CNode/页/TCB）从此
  无人回收，而 revoke 已经把水位线回卷，物理内存被立刻复用；
  ②`finalise_untyped` 删除派生 TCB 对象时**不清理调度器条目**，留下
  SUSPENDED 僵尸线程；陈旧 IPC 状态把它唤醒后，它就在已清零/复用的
  地址空间里执行（`user fault PC=0/0x18`，ESR 权限错误），并把新 init
  的栈踩烂——userboot 视其为 init 故障，销毁重启，循环往复。
  修复：①mark 改为从根（线程组 CSpace/VSpace + IRQ 绑定通知）做图遍历，
  CNode 只有自身可达时其 cap 才算根；②finalise 收集被删 TCB 的 task id，
  在 store 借还结束后逐个 `api::destroy` 清理调度器条目；
  ③新增 `TcbSuspendGroup`（rstiny 扩展标签 60，对齐托管路径 R::Destroy
  的组语义）：`Task::destroy` 销毁前挂起共享 CSpace 的全部成员，兄弟
  线程（init 的 logger）不再以运行态扎根组对象，成员在任何 revoke 之前
  全部停机。另修 check_fault_handler 的等待竞态（ready#3 与 started#3
  相邻落地，循环等待条件需与断言一致）。注意 SUSPENDED 仍算扎根
  （managed Suspend 的 repair 语义依赖对象存活），清理僵尸靠 ②。