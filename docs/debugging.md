# 调试工具（2026-09-16）

面向死锁/卡死/故障排查的观测设施。全部构建期可用，不需要额外服务。

## 1. 故障时自动输出（内核）

用户任务故障（user fault）时内核打印三段：

1. 故障行：任务 id、ESR、FAR、PC；
2. **用户态帧链回溯**：沿 x29 帧链的返回地址（原始地址）；
3. **任务表转储**：每个存活任务的 state / 优先级 / CSpace / VSpace /
   阻塞端点。内核 panic 时打印内核帧链 + 原始（无日志锁）任务表。

帧指针由 `.cargo/config.toml` 的 `-C force-frame-pointers=yes` 保证。

## 2. 符号化

```sh
python3 tools/symbolize.py --elf <kernel.elf> [--elf <app.elf> ...] <addr>...
```

## 3. 活体任务表转储（死锁第一现场）

```sh
python3 tools/task_dump.py                 # 默认镜像，跑 6 秒后转储
python3 tools/task_dump.py --delay 30     # 等待 30 秒（等卡死出现）
```

脚本经 gdbstub 停机，读内核 `SCHEDULER`/`Task` 的 DWARF 偏移，打印全部
任务的 state / CSpace / VSpace。配合 `screen`/日志观察哪个任务停在哪个
端点上，就是死锁的第一现场。

## 4. 其他

- QEMU gdbstub（`tools/check_kernel.py` 的 Gdb 类；`.text.probe` 段）。
- QEMU 自带追踪：`-trace enable=virtio*`；设备状态用 QMP
  `x-query-virtio-status`（查"设备被复位"类问题）。
- 惯例：`make check` 运行期间不要并行编辑源码或起停 QEMU。
