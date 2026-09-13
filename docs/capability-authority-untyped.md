# 能力唯一权威 + 资源皆 Untyped：设计与迁移规划

日期：2026-09-14。状态：C0–C3 已实施（C3 采用特性门控而非物理删除，见文末
[实施记录](#7-实施记录)）。实现与偏差以本记录为准。

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

（§1–§5 为设计文本,保留当时的事实与取舍;实施结果与偏差见下节。）

## 7. 实施记录（2026-09-14）

已实施：

- **C0（allocator 标准增长）**：`projects/libs/alloc` 的堆增长改为标准对象
  操作——`UntypedRetype` 从本任务自己的预算 cap（slot 32,服务链约定）切帧,
  `Page_Map`/`PageTable_Map` 装进自身 VSpace 的 `ALLOC_GROW_VA` 窗口,槽位用
  私有游标（`SLOT_BASE = 60_000`,位于 loader 窗口之上、16 位 CNode 之内;
  最初选的 96_000 超出 65536 槽上限,是落地时修掉的第一个错误）。invoke
  原语扩展到 6 词 + cap 的 IPC buffer 组包,与用户库同款。部分失败回滚
  （删除 cap 即解除映射,watermark 由 revoke 统一回卷）。预算从此必须包含
  堆余量:`init.cfg` 的 mysh 与 `init-appmgr.cfg` 的 appmgr 由 2M 提到 4M
  （镜像读取最大 512 KiB + mysh 的 1 MiB 每子进程预算）。userland 的
  `Runtime::Map` 调用点归零;`check_mysh`（4 内核变体 × 2 磁盘）、
  `check_python` 全绿。
- **C1（落账 + 删 managed_untyped + cap 强制）**：删除 `Store.managed_untyped`
  与 `collect` 的全区回卷;`map_vspace`/`create_vspace` 显式携带来源 Untyped
  （`None` 仅限内核 boot loader 自身地址空间）。`Runtime::Map`/`Create`
  在消息中强制携带 Untyped cap,帧、页表、被管子进程的 VSpace/TCB/CNode
  （名义 1 KiB + 64 KiB）全部计入该预算。`check_capabilities` 增加
  "无 cap 即无帧"负例;记账从"全局区自动回卷"改为区域粒度——只有 Revoke
  回卷 watermark,相关 harness 的泄漏断言先 revoke 再比较 `AvailableFrames`。
- **C2（用户态替代）**：loader `FindEmptySlot` → 槽游标（`elf.rs` 的
  `retype()` 直接吃 `loader_slot()` 游标）;`Task::destroy` → 挂起 + revoke
  loader 派生子树（标准 `CNode_Revoke/Delete`,组语义由派生关系自然覆盖）;
  `Task::destroy_thread` → `Tcb_Suspend` + 删除 TCB cap;`rstiny::sleep` →
  读 `Runtime::Clock` + `yield` 的用户态轮询（零 ms 仅让出一次）;
  `rstiny_runtime::protect_stack` 改走 `rstiny::unmap_self`（受限原语绑定,
  仅自身地址空间——root 栈守卫页是内核 boot 映射,没有 frame cap 可走
  标准路径）。
- **C3（Runtime 门控降级）**：内核 feature `managed-runtime`（Makefile
  `MANAGED=1`）。生产镜像编译掉 `Create/Start/Status/Wait/Destroy/
  DestroyThread/Cspace/Vspace/FindEmptySlot/Map` 十个标签,其余按四分法保留。
  `check_capabilities` 在生产内核上断言这些标签全部返回 `Unsupported`,
  信息类方法（Current/Clock/AvailableFrames）继续可用;cap 强制行为的负例
  移到 `check_tasks`（门控构建）。managed 回归套件
  （check_tasks/check_fpu/check_ipc/check_irq）用 `managed=True` 构建。

与原设计的偏差（保留的受限原语及理由）：

- `Exit`/`Shutdown` 保留（§3.1 允许"移为受限原语"）：二者均为自指——Exit
  只终止调用者（服务正常退出走 control_ep 协议 + 监督者 revoke,`rstiny::exit`
  已无应用调用者）;PSCI 是 EL1 监视调用,EL0 无法直接触达,只能保留为内核
  扩展而非用户态 power 服务。
- `Sleep` 保留为内核受限调度原语：自指、无资源;用户态 `rstiny::sleep`
  已不再依赖它（C2）,它只服务于调度器睡眠态语义与验收。用户态定时器服务
  （Generic Timer IRQ + Notification）仍是后续里程碑。
- `Unmap`/`Protect`/`WriteMemory`/`ReadMemory` 保留（§3.1 将其归入
  "改造成正常能力操作"而非删除）：它们不分配资源、不产生隐式取帧路径,
  目标必须由 WRITE TCB cap 指认且处于可编辑状态（self/停止/fault 阻塞,
  §3.3 的 editable/supervisor 语义）。root 栈守卫（Unmap）与监督者检视/
  修复路径（Write/Read）是现实用户;`Map` 因 C0 无任何调用者而被门控。
- C3 的"任何任务无法经 Runtime 影响他人"由生产内核上的
  `Unsupported` 断言 + 剩余方法的 cap/自指性质共同满足;`Object::Runtime`
  在生产镜像中退化为信息类 + 受限原语载体,托管能力全部位于
  `managed-runtime` 门控之内。

验收复跑：`make check` 全绿（managed 套件以 `MANAGED=1` 构建,其余全部为
生产内核）。