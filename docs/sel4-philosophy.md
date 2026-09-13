# 再探 seL4 核心设计理念：RSTiny 差异清单

对照本地 `../seL4` 内核树（`kernel/src/object/{untyped,cnode,endpoint,
notification,interrupt,tcb,objecttype}.c`、调度与控制块）重新核对的
seL4 设计理念，与 RSTiny **当前状态**（截至 2026-09，含 IPC/IRQ/设备栈/
services/mysh/MicroPython P3/FPU 上下文/`rstiny-alloc`）逐条对照。
本文只谈"理念/哲学层"差异；ABI 与对象层的细节差异见
[sel4-abi.md](sel4-abi.md)，实现差距与阶段见 [evolution-plan.md](evolution-plan.md)。

## 1. seL4 的核心设计理念（支柱）

1. **能力即唯一权威**（capability-safe）：没有整数化的"任务 ID 即权限"、
   没有内核内"全局服务"；一切操作先经 CNode 解析 cap，权限随 cap 的
   badge/rights/派生关系而定。
2. **资源皆从 Untyped 创造**：一切内核对象（TCB/CNode/VSpace/Endpoint/
   Notification/IRQHandler/调度上下文）的**物理内存都从用户划分的
   Untyped 区域 retype 而来**；内核自身不持有任何可运行期分配的资源池，
   用户内存耗尽与内核对象耗尽是同一个维度。
3. **机制与策略分离**：内核只给机制（cap、IPC、fault 投递、调度原语）；
   资源划分、进程组织、重启/监督策略全部由用户态 rootserver/CAmkES 决定。
4. **确定性与形式化验证**：实现级证明（Isabelle/HOL + C）；无 UB、无
   "未定义语义"；错误显式返回；fault 变成发给监督者的 IPC，绝不静默杀进程。
5. **最小 TCB**：驱动、文件系统、网络、日志全在核外；内核不是"厨房水槽"。
6. **IPC/fault 语义精确**：endpoint+badge、notification、ReplyRecv、
   fault endpoint——每个词都有精确语义和证明。

## 2. RSTiny 与各支柱的对照

| 支柱 | seL4 | RSTiny 现状 | 差距等级 |
| --- | --- | --- | --- |
| 能力唯一权威 | 无异径；对象表只由 cap 引用 | **存在 `Runtime` 管理对象**：loader 给每个子进程复制 `INIT_RUNTIME`（Current/Create/Start/Map/WriteMemory/Shutdown…），能力模型之外的一块"魔法服务" | **结构性差异** |
| 资源皆 Untyped | 对象物理内存从用户 Untyped 切 | 对象体在**全局 BTreeMap 对象表**，retype 按**名义记账**（TCB 1024B、CNode 每槽 1B）；有全局 `MAX_OBJECTS/MAX_CAPS` 元数据上限 | **结构性差异** |
| 机制/策略分离 | 策略在 rootserver | init/appmgr/mysh 已在用户态做监督与重启策略 | ✅ 大体一致 |
| 确定性/证明 | 实现级证明 | 无证明；靠 `check_*.py` QEMU 回归；内核仍有"review 才发现"的语义 bug（如 `Call` 缺 `GrantReply` 死锁、`assemble` 长度 121..=127 panic） | 明确放弃（登记） |
| 最小 TCB | 驱动/FS 全在核外 | 一致：console/block/fs/appmgr/shell/MicroPython 全用户态；FPU 惰性上下文进内核（合理，与未来 float 相关） | ✅ 一致 |
| IPC/fault 语义 | 证明覆盖 | endpoint/badge/ReplyRecv/fault 投递/Notification/IRQHandler 已对齐；精确性仍靠测试 | 待补齐 |

## 3. 逐条差异（重点，含根因）

### 3.1 `Runtime` 托管层：最需要正视的哲学差异

- seL4 没有"内核里的全能服务"。创建/启动/等待/映射/读写字存/关机这些
  要么是**用户的职责**（rootserver 持 Untyped cap 自己 retype+map、自己
  wait），要么是**用户态驱动**（PSCI 关机、电源管理）。
- RSTiny 的 `Runtime` 把这一整套做成内核对象并**发放给每个子进程**。
  直接后果（上一轮已查到）：`Runtime::Map` 从 `store.managed_untyped`
  （`init_root` 里设为用户态最大普通区）切帧，**任何持 Runtime cap 的任务
  都能白取 root 的帧**——记账错、能力模型穿洞。这正好是 seL4 支柱 1/2
  会直接否定的形态。
- 结论：evolution-plan 决策 E（删除/降级 Runtime 托管）应从"可选"提为
  "架构必修"；`rstiny-alloc` 的增长应改为**从任务自身 sub-Untyped
  retype 帧**（能力干净）或至少在 Runtime 保留期内把 `Map` 限权到 owner。

### 3.2 全局对象表 + 名义记账：资源语义的偏离

- seL4 对象实体（包括 cte、TCB 的物理存储）从用户 Untyped 切出，"内存
  耗尽"只有一个维度，且与用户划分严格对应。
- RSTiny 是"对象表（BTreeMap）+ 名义账单"：`TCB_BYTES=1KiB`、
  `CNODE_SLOT_BYTES=1B` 只是推进 Untyped watermark 的记账数字，对象体
  不在那区域里。于是：
  - 用户切多少物理内存，与内核还能建多少对象**解耦**（两个上限）；
  - "一个 CNode 到底占多少内存"没有真实物理语义，只是记账约定
    （文档 [untyped-plan.md](untyped-plan.md) 已注明）。
- 这不是功能缺陷，是**结构取舍**（Rust 对象表实现简单）。但要诚实：
  它没有继承 seL4"资源/权威/内存同一律"的哲学。

### 3.3 CNode 与能力寻址

- RSTiny：单层固定 16 位扁平 CNode（稀疏 BTreeMap）+ guard 数据；
  `Runtime::FindEmptySlot` 等便利仍在。
- seL4：任意深度 radix 树 CNode + guard，cap 派生树有完整删除/撤销语义。
- 已登记（sel4-abi.md）；属"待补功能"，非哲学分歧。

### 3.4 调度

- RSTiny：单核 FIFO + 10ms 时间片，无优先级（service-manager §17）。
- seL4 non-MCS：优先级 + 同优先级 round-robin；MCS 更进一步把
  "调度上下文的预算"纳入证明。
- RSTiny 演进计划把 `TCBSetPriority` 列为阶段 F；对教学内核是合理分期。

### 3.5 形式化验证（最大的单点差异）

- seL4 的全部哲学支点是"证明"：宁可少功能，也要语义可证。
- RSTiny 明确不继承证明（microkernel-design §1），用 `tools/check_*`
  QEMU 回归对冲。这是正当工程取舍，但要承认：
  - 因此"精确语义"没有证明背书，上一轮 review 的 IPC 缺陷就是证据；
  - 代价由测试语料承担 → 建议把发现的 IPC 缺陷补成回归（`check_ipc`），
    并保持"新机制必须有 check"的约定。

### 3.6 一致的部分（别只说差异）

- 能力对象的用户态引用（CapPtr 槽号，非整数即权）、设备 Untyped 只给
  驱动、IRQControl/IRQHandler 授权、fault 交给监督者、untyped
  retype/revoke、DMA 信任边界（受信 block server，无 IOMMU）、
  READY/EXIT/fault 监督协议、MicroPython 等应用当普通进程 —— 这些与
  seL4 模型同构，是 RSTiny 已经站对的地方。

## 4. 结论

1. **方向性差异有两条**，都来自"便利内核服务"的残留：
   - `Runtime` 托管层（含 `Runtime::Map` 的全局记账）违背
     "能力唯一权威 + 资源皆 Untyped"；
   - 全局对象表 + 名义记账，把"内存/对象"解耦成两个维度。
   建议分别以"决策 E 提优先级"和"对象实体尽量从 Untyped 切/或明确
   记为记账约定"两条处理。
2. **明确放弃的差异**：形式化验证。正当，但要用测试纪律补齐语义回归。
3. **待补功能（不是哲学分歧）**：CNode 深度、调度优先级、IPC 语义
   边界、MCS——已按阶段表演进。

引用链：[sel4-abi.md](sel4-abi.md)（ABI/对象）· [service-manager.md](service-manager.md)
（机制/策略与监督）· [untyped-plan.md](untyped-plan.md)（Untyped/记账）·
[evolution-plan.md](evolution-plan.md)（差距表与阶段）· [interpreter-app.md](interpreter-app.md)
（allocator/Runtime::Map 依赖）· [fpu.md](fpu.md)（FP/SIMD 上下文）。