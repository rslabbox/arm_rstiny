# Untyped 物理内存实现计划

日期：2026-09-10。状态：U1-U4 已实施，U5 部分实施。实现与偏差见文末[实施记录](#16-实施记录)。

本文把 [对象内存所有权模型](object-ownership.md) 里保留的简化项——"Untyped 仍是帧池分配权限的抽象"——升级为 seL4 语义的 Untyped：真实物理区间、watermark 切分、按区间计费、Revoke-all、设备内存和按对象类型的 finalisation。ABI 与对象方法见 [seL4 风格 ABI](sel4-abi.md)，总路线见 [能力系统与 IPC 演进规划](evolution-plan.md)，架构边界见 [内核地址空间](kernel-mapping.md)。

目标是**语义级 Untyped**：对象内存有真实的物理归属和预算，回收由 Untyped 撤销驱动。**不**要求对象 struct 原地放进 Untyped（完全级）；对象表继续作为内核元数据索引存在，但每个对象必须能回答"我来自哪个 Untyped、在哪个偏移"。

## 1. 为什么是下一步

当前对象表已经是单一所有者，但内存来源仍是：

- `Object::Untyped` 只是 `kernel/src/object/mod.rs` 里的一个标记；
- `retype` 通过 `Store::new_frame` / `new_page_table` / `new_vspace` 从全局 `POOL`（`kernel/src/memory/frame.rs`，8 MiB / 2048 帧）取页；
- `MAX_OBJECTS=4096`、`MAX_CAPS=8192` 是内核全局预算，不计入任何用户的资源；
- 设备内存没有授权路径，用户态驱动拿不到 MMIO；
- 回收依赖 mark-sweep `collect`，没有按对象类型的 finalisation。

Untyped 是"用户拥有物理内存"和"资源隔离"的共同承重件；不做它，capability 模型有保护、但没有预算，且设备内存与实时回收都无从落地。

## 2. 关键决策

| 选项 | 内容 | 取舍 |
| --- | --- | --- |
| **A. 语义级**（本计划） | Untyped 是真实物理区间 + watermark；对象记录归属；Revoke 重置区间并 finalize 子对象；对象表保留为内核索引 | 拿到 seL4 的计费/设备/确定性撤销语义，Rust 安全性基本保留 |
| B. 完全级 | 对象 struct 原地放在 Untyped 内，内核无独立对象表 | 最接近 seL4 的可验证形态，但大量 unsafe、改动巨大；本计划不做 |

选择 A。完全级作为独立里程碑，在需要形式化验证时再评估。

## 3. 现状与目标对照

| 维度 | 现状 | 目标 |
| --- | --- | --- |
| Untyped 语义 | 分配权限标记 | 物理区间 + watermark + `is_device` |
| 对象内存来源 | 全局 `POOL` | 父 Untyped 切分 |
| 计费 | `MAX_OBJECTS/MAX_CAPS` 全局 | 每个 Untyped 的剩余字节 |
| 设备内存 | 无 | 设备 Untyped 只能生成设备 Frame |
| 撤销 | 派生 cap 撤销；对象靠 GC 回收 | `Revoke(Untyped)` 重置区间并 finalize 子对象 |
| 回收 | 内核 mark-sweep | 显式 Revoke + 按类型 finalisation；GC 降为托管兜底 |
| 清零 | `Frame::allocate` 分配时清零 | 非设备 retype 清零 + 归还前清零 |
| BootInfo | 单个 Untyped 标记 | Untyped 描述列表 + 区间 cap |

## 4. 数据结构

### 4.1 Untyped 对象

建议新增 `kernel/src/object/untyped.rs`：

```rust
pub struct Untyped {
    phys: PhysAddr,
    size_bits: u8,        // 12..=30，区间大小 2^size_bits
    is_device: bool,
    free_offset: usize,   // watermark，0..=size
    parent: Option<ObjectId>, // 由更大的 Untyped 切出时记录
}
impl Untyped {
    pub fn size(&self) -> usize { 1usize << self.size_bits }
    /// 从 watermark 起做对齐切分；失败不改变状态。
    pub fn allocate(&mut self, size: usize, align: usize) -> Option<PhysAddr> {
        let base = self.phys.as_usize();
        let start = (base + self.free_offset).checked_add(align - 1)? & !(align - 1);
        let end = start.checked_add(size)?;
        if end > base.checked_add(self.size())? {
            return None;
        }
        self.free_offset = end - base;
        Some(PhysAddr::from_usize(start))
    }
    pub fn reset(&mut self) { self.free_offset = 0; }
    pub fn remaining(&self) -> usize { self.size() - self.free_offset }
}
```

约束：

- 区间本身 2 的幂且按其大小对齐（seL4 的 Untyped 不变量）。
- `allocate` 只推进 watermark；单个对象不能单独释放，只能整段 `reset`。这是 seL4 的语义，也是确定性回收的来源。
- 设备 Untyped 只允许 `size_bits >= 12`，且只生成 Frame。

### 4.2 对象归属

对象表槽需要能回答"父 Untyped + 偏移"。建议扩展 `kernel/src/object/id.rs` 的 `Slot`：

```rust
struct Slot {
    generation: u32,
    next_free: u32,
    object: Option<Object>,
    owner: Option<ObjectOwner>,   // 新增
}
#[derive(Clone, Copy)]
pub struct ObjectOwner {
    pub untyped: ObjectId,
    pub offset: usize,
    pub size: usize,
}
```

`insert` 增加 `owner` 参数；`insert` 的调用者（`retype`）传入父 Untyped 与偏移。`ObjectTable` 增加：

```rust
pub fn children(&self, untyped: ObjectId) -> impl Iterator<Item = (ObjectId, ObjectOwner)> + '_;
```

第一版 `children` 直接遍历槽（O(对象数)），完整 MDB 子树留待后续。

### 4.3 与现有 Store 的关系

`Store` 仍是对象表 + CSpace + 派生记录的组合。新增的是：

- `Object::Untyped(Untyped)` 携带区间；
- 每个对象的 `ObjectOwner`；
- `Frame` 增加"从 Untyped 构造"的入口，取代 `Frame::allocate` 的用户路径。

`MAX_OBJECTS/MAX_CAPS/MAX_DERIVATIONS` 保留，但语义改为**内核元数据上限**，与用户内存预算分开表述。

## 5. BootInfo v5

### 5.1 头部

`projects/libs/abi/src/lib.rs` 升到 `ABI_VERSION = 5`：

```rust
#[repr(C)]
pub struct BootInfo {
    pub magic: u64,
    pub version: u64,          // 5
    pub size: u64,
    pub page_size: u64,
    pub flags: u64,
    pub ipc_buffer: u64,
    pub extra: u64,
    pub extra_size: u64,
    pub untyped_start: u64,    // 第一个 Untyped cap 槽号
    pub untyped_count: u64,
    pub reserved: [u64; 6],
}
```

`untyped_start` 起连续 `untyped_count` 个槽由内核预置 Untyped cap。现有 `INIT_UNTYPED = 16` 标记在 v5 中废弃（见第 11 节迁移）。

### 5.2 扩展记录

沿用 id/len 头：

| id | 记录 | 布局 |
| --- | --- | --- |
| 6 | FDT | 现有 |
| 7 | Untyped 列表 | `count: u64` + `UntypedDesc[count]` |

```rust
#[repr(C)]
pub struct UntypedDesc {
    pub paddr: u64,
    pub size_bits: u64,   // 12..=30
    pub is_device: u64,   // 0/1
    pub reserved: u64,
}
```

读取规则保持"已知字段向后兼容、未知记录跳过"，与现有扩展区一致。

## 6. 启动期 RAM 切分

### 6.1 保留区

在 `kernel/src/boot.rs` 现有保留区基础上，明确排除：

- 内核镜像物理范围（`memory::address::kernel_image()`，含 text/rodata/data/bss/stack/heap/frames）；
- loader 自身范围（固定平台 `0x4400_0000` 起，交接后是否可回收单列一项）；
- DTB 物理范围；
- root image 及其程序头页（已作为 root 的 Frame，不进 Untyped）；
- 平台保留区（固件前 2 MiB 等）；
- 设备范围：GIC、timer 不进用户 Untyped；UART/VirtIO 进**设备 Untyped**。

### 6.2 2 的幂切分

对每个剩余空闲区间做一次对齐切分，产出 Untyped 列表：

```text
for region in free_regions:
    cursor = align_up(region.start, PAGE_SIZE)
    while cursor < region.end:
        # 取不超过剩余长度、且按大小对齐的最大 2 的幂
        size = largest_power_of_two <= (region.end - cursor)
        size = min(size, align_down_power_of_two(cursor))
        emit Untyped(paddr = cursor, size_bits = log2(size))
        cursor += size
```

可选：为减少 Untyped 数量，先合并相邻同尺寸块。第一版以"正确、可枚举"为准，不做合并。

### 6.3 交给 root

`init_root` 不再插入单个 `INIT_UNTYPED`，而是在 `untyped_start..untyped_start+count` 为每个描述插入一个 `Object::Untyped` cap，并把 `untyped_start/count` 写进 BootInfo。`Runtime::AvailableFrames` 改为所有用户 Untyped 剩余字节之和（设备不计入可用帧）。

## 7. Retype 流程

`kernel/src/object/invoke.rs::retype` 改为：

1. 校验 Untyped cap（类型 + `Write`）、`type/sizeBits/count`、目标槽空、容量。
2. 设备 Untyped 只允许 `SmallPage`；普通 Untyped 允许 `TCB/CNode/VSpace/SmallPage/PageTable`。
3. 保存 `watermark_before = untyped.free_offset`。
4. 对每个对象：
   - `untyped.allocate(object_size, object_align)`；失败返回 `NotEnoughMemory`；
   - 非设备：清零该范围；
   - 构造 payload：Frame/PageTable 用 `Frame::from_untyped(phys)`；VSpace 用切分出的三个页表页；CNode/TCB 的元数据仍在内核对象表，但记录归属；
   - `ObjectTable::insert` 写入 `ObjectOwner { untyped, offset, size }`。
5. 插入 cap，`parent = untyped_cap.serial`。
6. 任一步失败：`untyped.free_offset = watermark_before`，删除已插入对象（`ObjectTable::remove` 触发 `Frame::drop` 归还帧池——注意此时帧池不再拥有这些页，归还应回到 Untyped，见下）。

**关键点**：一旦对象内存来自 Untyped，`Frame::drop` 不能再把它还给全局 `POOL`。改为：

- `Frame` 只记录地址，不负责归还；
- 归还由 `finalise`/`reset` 完成；
- 全局 `POOL` 只服务 boot 期初始对象（root TCB/CNode/初始页表/BootInfo 页），用户路径不再调用。

这是本次改动里最容易遗漏的一致性点：**"谁切分谁回收"**。

## 8. Revoke(Untyped) 与 finalisation

### 8.1 语义

`Revoke(Untyped)`：

1. 校验 Untyped cap。
2. 收集该 Untyped 的全部子对象（`ObjectTable::children`，含由它切出的子 Untyped，递归）。
3. 对每个子对象执行类型化 finalisation（见 8.2），并按派生关系删除其 cap。
4. 非设备区间清零。
5. `untyped.reset()`，watermark 归零。
6. 若保留托管 GC，`request_collect()`。

### 8.2 finalisation 清单

| 对象 | finalise |
| --- | --- |
| Frame | 扫描持有该对象的 cap，解除其 `mapping`；TLB 失效；非设备清零 |
| PageTable | 要求表中无页；解除映射；TLB 失效 |
| VSpace | 解除全部页与页表映射；TLB 失效；丢弃 `AddressSpace` |
| CNode | 递归删除槽内全部 cap（含其 mapping 与派生后代） |
| TCB | 从调度器移除、释放内核栈、清理等待关系 |
| Untyped（子） | 递归 Revoke 后归还父区间 |

顺序：先解除映射与 TLB，再删 cap，最后重置区间。设备 Frame 不做清零。

### 8.3 与现有 collect 的关系

`collect` 暂时保留，但**只作为托管 `Runtime` 的兜底**。标准对象路径应能只靠 `Revoke/Delete` 完成回收。U5 阶段评估是否移除 `collect`，或把它限制在 `managed` 集合内。

## 9. 清零策略

| 时机 | 普通 Untyped | 设备 Untyped |
| --- | --- | --- |
| retype 分配后 | 清零 | 不清零 |
| finalise 归还前 | 清零 | 不清零 |
| 子 Untyped 归还父区间 | 清零 | 不清零 |

目的：跨任务复用不泄漏上一个对象的数据。设备内存不清零是语义要求（保留固件/设备状态），但只允许映射为 Device/NX。

## 10. 设备 Untyped

- 平台配置（`kernel/src/config.rs` 及生成平台常量）声明设备区间：UART、VirtIO MMIO；GIC、timer 保留给内核。
- boot 切分时设备区间生成 `is_device = true` 的 Untyped。
- `retype` 设备 Untyped 只允许 `SmallPage`。
- 设备 Frame 的映射使用 Device 属性、`UXN`，拒绝 `EXECUTE` 与普通 cacheable 属性。
- 用户态驱动流程：root 从设备 Untyped retype Frame → 复制 Frame cap 给驱动 → 驱动 `ARM_Page_Map` 到自己的 VSpace → 驱动访问 MMIO。
- 无 IOMMU/SMMU 时，持有总线主控设备的驱动属于受信任组件，文档必须保留该边界。

## 11. 迁移与兼容

| 现有 | v5 之后 |
| --- | --- |
| `INIT_UNTYPED = 16` 单个标记 | 废弃；改用 `untyped_start..+count` 区间 cap |
| `Frame::allocate` 用户路径 | 仅 boot 期使用；用户 Frame 来自 Untyped |
| `Runtime::AvailableFrames` | 所有用户 Untyped 剩余字节之和 |
| `MAX_OBJECTS/MAX_CAPS` | 内核元数据上限，与用户内存预算分开 |
| `collect` mark-sweep | 托管兜底；标准对象走 Revoke/Delete |
| `projects/libs/user/src/elf.rs` 的 `INIT_UNTYPED` | 改为遍历 BootInfo 的 Untyped 列表，挑一个足够大的区间 |

ABI 从 v4 升到 v5，旧用户程序不保证可运行；fatboot 与 `rstiny` 用户库同步更新。

## 12. 分阶段实施

### U1：BootInfo v5 + 启动分区 + root Untyped cap

- 目标：root 能枚举真实 Untyped。
- 改动：`projects/libs/abi`（BootInfo v5、`UntypedDesc`、扩展记录 id=7）；`kernel/src/boot.rs`（保留区与切分）；`kernel/src/object/mod.rs`（`init_root` 插入区间 cap）；`projects/libs/runtime`（BootInfo 视图）。
- 验收：root 枚举到非空列表；GIC/timer 不在列表；UART/VirtIO 标记为设备；`available_frames` 由 Untyped 总量推导；扩展记录边界校验。
- 风险：切分算法漏掉对齐/重叠；保留区遗漏导致内核内存被当成空闲。

### U2：Untyped watermark + retype 切分

- 目标：用户对象内存全部来自 Untyped。
- 改动：新增 `object/untyped.rs`；`object/id.rs` 的 `Slot.owner`；`object/invoke.rs::retype`；`memory/frame.rs`（`from_untyped`、移除用户路径的 `allocate`）；`object/mod.rs`（`new_frame/new_page_table/new_vspace` 改为从 Untyped）。
- 验收：耗尽返回 `NotEnoughMemory`；同一 Untyped 多次 retype 线性推进；失败回滚 watermark；用户 Frame 不再消耗全局 `POOL`。
- 风险：`Frame::drop` 与 Untyped 归还的所有权冲突（见第 7 节）。

### U3：对象归属 + Revoke(Untyped) + finalisation + 清零

- 目标：显式撤销即可完整回收。
- 改动：`object/id.rs`（`children`）；`object/cnode.rs`（Revoke 走 Untyped 子对象）；`object/invoke.rs`（`Untyped_Revoke` 标签；seL4 中 Revoke 作用在 Untyped cap 上）；`memory/space.rs`（批量解除映射）；`api/faults.rs`（finalisation 期间不产生用户故障）。
- 验收：Revoke 后旧 cap 失效；帧回到 Untyped；页表/TLB 已清；复用内存不泄漏旧数据；设备内存不被清零；子 Untyped 递归回收。
- 风险：finalisation 顺序错误导致"内存已复用、旧映射仍在"；这与现在的 GC 路径并存时容易混淆。

### U4：设备 Untyped

- 目标：用户态驱动可拿 MMIO。
- 改动：`kernel/src/config.rs` / 平台生成（设备区间）；boot 切分标记 `is_device`；`object/invoke.rs`（设备 Untyped 只允许 Frame）；`memory/space.rs`（Device/NX 映射）；`arch/machine` 保留 GIC/timer。
- 验收：用户任务能 retype 设备 Frame 并映射；普通任务拿不到 GIC/timer；设备映射不可执行；无 IOMMU 的信任边界写入文档。
- 风险：设备区间与内核保留区重叠；属性配置错误导致 cache 一致性问题。

### U5：回收策略移到用户态，GC 降级

- 目标：内核只做机制，用户态分配器决定 retype/revoke。
- 改动：`projects/libs/user` 增加 allocman 式分配器（从 BootInfo Untyped 列表切分、记账、Revoke）；`Runtime` 托管层标注为过渡；评估移除或隔离 `collect`。
- 验收：反复 create/revoke 不依赖内核 GC 也能稳定回收；`available_frames` 与用户态账本一致；托管与标准两套路径的边界清晰。
- 风险：内核 GC 与用户 Revoke 并存导致双重回收或漏回收。

## 13. 测试与验证

| 层 | 检查 |
| --- | --- |
| 宿主单元 | Untyped 对齐/耗尽/回滚算术；启动切分算法（保留区、重叠、2 的幂）；BootInfo v5 wire fixture；`UntypedDesc` 布局 |
| QEMU 集成 | root 枚举 Untyped；从 Untyped retype 各类对象；耗尽 `NotEnoughMemory`；失败回滚；`Revoke(Untyped)` 后旧 cap 失效、内存复用不泄漏；设备 Untyped 只允许 Frame；GIC/timer 不在列表；`available_frames` 与账本一致 |
| 回归 | 现有 `tools/check_capabilities.py`、`check_tasks.py`、`check_user_context.py`、`check_fatboot.py`、`check_relocation.py` 全绿 |
| 新增 | 建议 `tools/check_untyped.py`：枚举、切分、耗尽、回滚、Revoke、设备、清零 |

每个阶段同时提供正常用例和权限/失败用例；finalisation 要验证"内存复用不泄漏旧数据"和"TLB 已失效"。

## 14. 明确不做与延后

- 完全级 in-place 对象（对象 struct 放进 Untyped）。
- 完整 MDB 子树撤销（第一版用对象表扫描）。
- Untyped 的任意拆分/合并（`Retype(Untyped → Untyped)`）。
- 设备 DMA 静止检查与 IOMMU/SMMU。
- SMP 下的对象表加锁。
- 形式化验证。

## 15. 里程碑

| 阶段 | 交付 | 依赖 | 退出条件 |
| --- | --- | --- | --- |
| U1 | BootInfo v5 + 启动分区 + root Untyped cap | 现有对象表 | root 能枚举真实 Untyped |
| U2 | watermark + retype 切分 | U1 | 用户对象内存来自 Untyped，耗尽/回滚正确 |
| U3 | 归属 + Revoke + finalisation + 清零 | U2 | 显式 Revoke 完整回收，无泄漏 |
| U4 | 设备 Untyped | U3 | 用户驱动可映射 MMIO |
| U5 | 用户态分配器，GC 降级 | U4 | 不依赖内核 GC 稳定回收 |

建议顺序：U1 单独提交并冻结 BootInfo v5 布局；U2/U3 一起评审，因为"切分"和"回收"必须成对；U4 紧随其后（设备内存是用户态驱动的门票）；U5 在标准路径稳定后再做。

## 16. 实施记录

已实施：

- **BootInfo v5**：`BootInfo` 增加 `untyped_start`/`untyped_count`/`reserved[6]`，扩展记录 id=7 携带 `UntypedDesc` 列表；`ABI_VERSION = 5`。初始 Untyped cap 区间从 `INIT_UNTYPED = 32` 起。
- **启动分区**：`kernel/src/boot.rs::boot_regions` 从 RAM 中排除固件、内核镜像、DTB、root 镜像与程序头页、bootloader 固定区间，再按 2 的幂对齐切分；UART 作为设备 Untyped 追加。内核直接映射改为覆盖固件之上全部 RAM（镜像别名保留原权限），否则 Untyped 页不可访问。
- **watermark 与归属**：`object/untyped.rs` 的 `Untyped` 实现对齐切分与 `reset`；`ObjectTable` 槽记录 `ObjectOwner { untyped, offset, size }`；`retype` 用 `Untyped::fits` 精确模拟 watermark 再一次性提交，失败不推进 watermark。
- **计费**：TCB/CNode 也推进 watermark（TCB 1024 字节、CNode 每槽 1 字节），`MAX_OBJECTS/MAX_CAPS` 因此是纯元数据上限；托管 `Runtime` 的 VSpace/页表/页同样从最大普通 Untyped 切分，`collect` 在该区间无子对象时重置它。
- **所有权**：`Frame::from_untyped` 的 `Drop` 不再归还帧池，回收由 `Untyped::reset` 完成；非设备页在分配时清零。
- **Revoke**：`CNode_Revoke` 作用于 Untyped cap 时递归 finalise 子对象、清零并重置 watermark；旧 cap 随后失效。
- **设备**：设备 Untyped 只允许 `SmallPage`；GIC/timer 不进入 Untyped 列表。
- **验证**：`tools/check_untyped.py` 覆盖枚举、计费、设备策略与区域级 Revoke；既有 `make check` 全绿。

保留的简化（与本文原设计的差异）：

- TCB/CNode 的元数据仍在对象表，但按固定消耗量计费：TCB 1024 字节（1024 对齐）、CNode 每槽 1 字节（页对齐，`CNODE_SLOT_BYTES`）。扁平 CNode 是稀疏 `BTreeMap`，每槽实际只占几字节，旧值 8 字节/槽会让 16 位 CNode 预定 512 KiB，五个服务的子 untyped 因此放不进 init 的 16 MiB 预算；1 字节/槽给出单调、非任意的价格（满 16 位 CNode = 64 KiB）。它们推进 watermark，因此 `MAX_OBJECTS/MAX_CAPS` 退化为纯元数据上限。
- 托管 `Runtime` 层也从同一 Untyped 切分（`create_vspace`/`map_vspace` 使用内核选定的最大普通区间）；当该区间不再拥有任何子对象时，`collect` 重置并清零它，因此 `POOL` 只服务 boot 对象。`AvailableFrames` = 全部普通 Untyped 剩余。
- 尚未提供用户态 allocman 式分配器（U5）；`Revoke` 已能独立回收，但用户库仍直接使用单个 Untyped cap。
- 子 Untyped（`Retype(Untyped → Untyped)`）、设备 DMA 静止检查与 IOMMU 未实现。

