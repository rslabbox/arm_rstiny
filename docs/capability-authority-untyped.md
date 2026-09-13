# 能力唯一权威 + 资源皆 Untyped：设计与迁移规划

对应 [sel4-philosophy.md](sel4-philosophy.md) 的两条结构性差异（`Runtime`
托管层、全局对象表 + 名义记账），给出**落地设计**与**迁移阶段**。目标:
让 RSTiny 满足 seL4 支柱 1（能力即唯一权威）与支柱 2（资源皆从 Untyped
创造）在"教学内核可负担"尺度下的判定，同时不重写整个内核。

## 1. 判定标准（先定"做到什么算达成"）

**支柱 1（能力唯一权威）**
- 不存在"无 cap 即得权"的入口;任何对象操作经 CNode 解析 cap。
- 内核不提供"全能便利服务":Create/Map/WriteMemory 等要么是普通对象方法
  （cap 验证所有权），要么不存在。
- 需要验证的入口：`Runtime::*` 全部方法、`Tcb_SetSpace/Configure` 的目标
  权限、`ArmVspaceTranslate`（DMA 自翻译，仅限本人 VSpace）。

**支柱 2（资源皆 Untyped）**
- 每个对象可追溯到**具体来源 Untyped**(来源、偏移、大小),不是全局池。
- 内核没有"可运行期喂给任意任务的单例资源"(删除 `managed_untyped`)。
- 用户内存耗尽与"还能建多少对象"由同一 Untyped watermark 决定;`MAX_OBJECTS`
  只作元数据护栏,不作资源语义。

## 2. 现状盘点（事实）

### 2.1 Runtime 方法 × 用途 × 使用方（2026-09 代码事实）

| 类别 | 方法 | 使用方 |
| --- | --- | --- |
| 信息类 | `Clock` / `AvailableFrames` / `DebugConsoleAvailable` / `Current` | init/appmgr/mysh 睡眠退避、可用帧、debug 控制台、`suspend_self`/alloc 找自身 TCB |
| 资源分配类 | `Map`(区域映射,隐式从 managed untyped 切帧) | `rstiny-alloc` 堆增长 |
| 控制类 | `Create/Start/Wait/Status/Destroy/DestroyThread/WriteMemory/ReadMemory/Unmap/Protect/Cspace/Vspace` | managed `Task` API(userboot/init/appmgr/mysh 的过渡路径) |
| 槽位便利 | `FindEmptySlot` | `elf.rs` loader 找 loader 槽位 |
| 时间/退出 | `Sleep` / `Exit` / `Shutdown` | `rstiny::sleep`(init/appmgr/mysh)、hello/minic 的 EXIT、mysh `exit`→PSCI |

### 2.2 对象存储/记账现状

- 对象体在全局 `ObjectStore`(BTreeMap),`ObjectOwner{untyped,offset,size}`
  已提及但未落账(untyped-plan.md)。
- `ObjectStore.managed_untyped` 由 `init_root` 设为**最大普通区**,`Runtime::Map`
  从它隐式取帧 → 上一轮评审已确认"记账错 + 能力穿洞"。
- 名义记账:`TCB_BYTES=1KiB`、`CNODE_SLOT_BYTES=1B` 推进 Untyped watermark,
  但对象实体不在该区域;`MAX_OBJECTS/MAX_CAPS` 是独立上限。

## 3. 设计

### 3.1 Runtime 方法四分法（核心决策）

| 处置 | 方法 | 说明 |
| --- | --- | --- |
| 保留（纯信息、限权） | `Clock`、`AvailableFrames`、`DebugConsoleAvailable`、`Current` | 不产生资源/控制效果;同一任务只查询自身/全局只读信息 |
| 改造成正常能力操作 | `Map`/`Unmap`/`Protect`/`WriteMemory`/`ReadMemory`（见 3.2 与 3.3） | 所需帧/目标必须由调用者以 cap 显式给出;所有权经验证 |
| 移入用户库/init | `Create/Start/Wait/Status/Destroy/DestroyThread/Cspace/Vspace/FindEmptySlot` | 见 3.4；loader 与 Task API 全部用标准对象方法重写 |
| 移为受限原语/用户态驱动 | `Sleep`、`Exit`、`Shutdown` | Sleep 由用户态定时器驱动（阶段 F2）;Exit 本就是子进程对 control_ep 的协议;Shutdown 移 PSCI 到专用 power 服务(或保留为仅 root 可调) |

### 3.2 allocator 改为标准 Untyped 增长（先行,利益最大）

- 现在:`rstiny-alloc` 用 `Runtime::Current` + `Runtime::Map` 从全局 managed
  区增长。
- 改为:allocator 向调用者要一个 **Untyped cap**(loader 给子进程的 slot 32
  sub-Untyped,已存在),增长时
  `UntypedRetype(SmallPage)` + `Page_Map(自身 VSpace, VA, RW/NX)`(标准对象
  操作,用户库已封装 `retype_at/page.map` 同款)。VA 使用 `ALLOC_GROW_VA` 段,
  记账自然落在该 Untyped 的 watermark 上。
- 收益:① 记账归子进程自己的预算;② `Runtime::Map` 的隐患随之消失;③ 为
  "删 managed_untyped"铺路。

### 3.3 Map/WriteMemory 等控制类:cap+所有权强制

- 原则:任何"改目标任务/目标 VSpace"的操作,调用者必须持有指向目标对象的
  cap(**所有权与 target 一致**,或经监督者的 fault 修复路径——后者已由
  `editable/supervisor` 语义覆盖)。
- 具体:
  - `Map(tcb, va, len, attr)` → 调用者须持有目标 TCB 或其 VSpace 的 cap,
    且帧来自调用者显式提供的 Untyped(3.2);
  - `WriteMemory/ReadMemory` 同理(seL4 没有这两个方法,本就该删/降级)。
- 由此 `managed_untyped` 字段可删除;`init_root` 不再设置全局资源。

### 3.4 移入用户库的替代实现

| 现 Runtime | 用户态替代 | 依赖 |
| --- | --- | --- |
| `FindEmptySlot` | `CNode::find_empty`(在自 CSpace 的槽区间线性扫描,loader 已有 `LOADER_SLOT_BASE` 窗口约定) | 无内核改动 |
| `Create/Start/Wait/Destroy` | `Task` API 改为封装"标准 loader 路径":retype TCB→Configure→WriteRegisters→Resume 与 fault/EXIT 回收(userboot/init 已有 `spawn_supervised`) | 无内核改动 |
| `Cspace/Vspace` | 由正常的 TcbConfigure/对应 cap 持有者得知 | 无内核改动 |
| `Sleep` | 用户态定时器(Generic Timer + `Runtime::Clock` 或 Notification),init/appmgr/mysh 的退避改用 `Task::sleep`(库实现) | 阶段 F2 |
| `Shutdown` | PSCI 移入用户态 power 服务(如 console 同款受信服务),或保留为仅 root 可调的原语 | 阶段 F2 |

### 3.5 对象记账落账（支柱 2 的中间态,不重写对象存储）

- 阶段 F1 先实现 `ObjectOwner` 落账:每个对象记录来源 Untyped + 偏移 + 大小;
  - `Retype` 必须显式带来源 Untyped cap(已是要求),记账精确到该 Untyped;
  - `Revoke/Delete` 按来源回卷 watermark(已有 untyped.reset,补充来源一致性);
  - `Managed untyped` 删除后,`Runtime::*` 没有任何隐式资源路径可验证。
- 记账数字口径统一:`TCB_BYTES/CNODE_SLOT_BYTES` 保留为"名义字节"并在文档
  注明是记账约定(untyped-plan.md 已如此),但**必须附在来源 Untyped 上**,
  不能再是全局名义。

## 4. 分阶段(对齐 evolution-plan 风格)

| 阶段 | 内容 | 验收 |
| --- | --- | --- |
| F0 | 盘点冻结 + allocator 改标准 Untyped 增长(3.2) | `check_mysh`/`check_python` 帧数断言不变;`Runtime::Map` 调用点归零(userland) |
| F1 | 对象落账 ObjectOwner;删 `managed_untyped`;`Map/WriteMemory/ReadMemory/Unmap/Protect` 改为 cap+所有权强制(3.3) | `check_untyped`/`check_capabilities` 全绿;新"无 cap 取帧"权限用例(负例)加入 `check_capabilities` |
| F2 | 用户态替代落地:loader FindEmptySlot→CNode 扫描;Task API→标准 loader;Sleep→定时器;Shutdown→power 服务(3.4) | 全 `make check` 通过且 grep 确认 userland 不再引用被移除的 Runtime 方法 |
| F3 | `Runtime` 对象降级/删除(特性门控),`Object::Runtime` 若保留仅信息类 | fuzz/回归:任何任务无法经 Runtime 影响他人;文档状态翻转为"已移除" |

## 5. 风险与不做项

- 不做:重写对象存储为"对象实体物理驻 Untyped"(F3 远期可另立里程碑);
  不为"最低限度可行性"牺牲 CNode 深度/调度(那是 evolution-plan 阶段 F)。
- 风险与缓解:
  - loader 从 FindEmptySlot 改为槽扫描:槽区间小(loader 窗口),线性成本可忽略;
  - Task API 重写可能牵动 init/appmgr/mysh:改由 `spawn_supervised` 统一,
    回归靠 `check_fault_handler`/`check_restart`;
  - Sleep 用户态定时器:需 Generic Timer IRQ 授权(irq 链已有),列为 F2 前置。

## 6. 联动

- 设计源:[sel4-philosophy.md](sel4-philosophy.md) §3.1/3.2;
- 记账/Untyped:[untyped-plan.md](untyped-plan.md);
- allocator 依赖:[interpreter-app.md](interpreter-app.md) 决策 B;
- 阶段总表:[evolution-plan.md](evolution-plan.md)(把 F0–F3 并入);外设的
  定时器/IRQ 见 [irq.md](irq.md)。

（本文为规划文档:阶段是否实施、实施顺序由后续决策/PR 决定,不在本文承诺。）