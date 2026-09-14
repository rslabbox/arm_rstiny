# seL4 风格 ABI 与内核对象

设计理念层的对照（能力唯一权威、资源皆 Untyped、机制/策略分离、形式化验证等）见 [sel4-philosophy.md](sel4-philosophy.md)。本文只讲 ABI/对象层。

当前实现对齐 seL4 **AArch64、non-MCS、单核、无硬件调试/SMMU/VCPU** 配置的调用协议及部分对象方法，不是完整的 seL4 二进制兼容实现。参考本地 `../seL4/kernel`，提交 `28b8f4c40d4a48a206bedc7875c6695f6106f34b`。

协议依据为 `libsel4/include/api/syscall.xml`、`libsel4/include/interfaces/object-api.xml`、ARM/AArch64 的 `object-api-arch.xml`、`object-api-sel4-arch.xml`，以及 AArch64 `syscalls.h` 和 64 位 `shared_types.bf`。对象调用标签由 XML 顺序和配置共同决定，不能把本表应用于任意 seL4 构建。

## 调用边界

用户通过 `svc #0` 陷入，x7 为有符号系统调用号：

| 调用 | 号 | 当前行为 |
| --- | --- | --- |
| Call | -1 | 调用 x0 capability 指向的内核对象；endpoint 上即阻塞 Call |
| ReplyRecv | -2 | 先消费回复关系，再在 x0 cap 上阻塞接收 |
| Send / NBSend | -3 / -4 | endpoint/notification 发送；NBSend 无等待者时为成功空操作 |
| Recv / Reply | -5 / -6 | endpoint/notification 阻塞接收；Reply 回复当前 caller |
| Yield | -7 | 让出 CPU，保留用户通用寄存器 |
| NBRecv | -8 | 非阻塞接收；无消息时返回 badge 0、空 tag |
| DebugPutChar | -9 | 输出 x0 低字节；LOG=off 时不可用 |

Endpoint 语义（non-MCS）对齐 seL4：发送方入队（Send/Call）或直接交付给已等待的接收方；一次调用建立一次性回复关系，Reply 消费它。与 seL4 的两处明示偏离见下文与"尚未对齐的部分"：Call 无 GrantReply 立即报错（本节），`Runtime::Sleep` 保留为内核受限原语（"显式运行时扩展"一节）。

Call 输入为 x0=CPtr、x1=MessageInfo、x2..x5=前四个消息字。更多消息字从当前任务登记的 IPC buffer 读取；额外 capability 也从该 buffer 读取。CPtr 总是在调用者的 CSpace 中解析，不是全局任务 ID。

MessageInfo 的 bit 0..6 为 length、7..8 为 extraCaps、9..11 为 capsUnwrapped、12..63 为 label；最多 120 个消息字、3 个额外 capability。请求不接受 capsUnwrapped。调用者提供的 tag 在 syscall 边界统一经 `MessageInfo::valid` 校验（对象调用与 endpoint IPC 共用这一个检查，2026-09 P0.1）：length 超过 120 或 capsUnwrapped 非零立即以 `TruncatedMessage` 拒绝，发送方得到显式错误——内核不会组装越界消息，也不会让非法 tag 进入等待队列。返回 x0=0，x1 为回复标签及长度，返回值放在 x2 开始的消息寄存器中；x6..x30 保留。当前对象方法只返回零或一个消息字。

IPC buffer 大小与对齐均为 1024 字节：tag 在 0，120 个 msg 在 8，userData 在 968，三个 caps/badges 在 976，receiveCNode/index/depth 在 1000/1008/1016。用户库将 TPIDRRO_EL0 作为 IPC buffer 地址来源，这是本项目的运行库约定，不能视作通用 libsel4 TLS 兼容保证。内核每次进入用户态重新设置它；用户库不创建跨 syscall 的可变 buffer 引用。

错误放在回复 MessageInfo.label：0=NoError、1=InvalidArgument、2=InvalidCapability、3=IllegalOperation、4=RangeError、5=AlignmentError、6=FailedLookup、7=TruncatedMessage、8=DeleteFirst、9=RevokeFirst、10=NotEnoughMemory。当前错误消息只附一个零字，没有实现 libsel4 各错误的完整附加字段。

## 对象、CSpace 与生命周期

初始任务拥有 TCB=1、CNode=2、VSpace=3、ASIDPool=6、IPCFrame=10；本项目额外提供 Untyped=16、Runtime=17，空闲槽从 32 开始。16/17 的分配与 BootInfo 均为本项目约定。CSpace 是固定 16 位槽索引的稀疏表，调用深度为 64，配置的根 guard 数据为 48；暂不支持任意嵌套 CNode 或 guard。

capability 引用对象，并携带派生关系、权限及该 cap 的映射记录。Copy 创建派生 cap；Frame cap 可以衰减读写权限。Move 保留派生身份和映射记录。Delete 删除一个 cap；Revoke 删除它的所有后代，跨 CSpace 生效，但保留被调用的源 cap。对象表是全部对象 payload 的唯一所有者，capability 与地址空间只保存 `ObjectId`/`FrameRef`；回收是按需触发的标记-清除，页帧在没有任何 capability 或地址空间引用它之后才归还帧池。所有权模型详见 [对象内存所有权模型](object-ownership.md)。

| 对象方法 | 标签 | 请求消息字 / 额外 cap |
| --- | --- | --- |
| Untyped_Retype | 1 | type, sizeBits, nodeIndex, nodeDepth, offset, count / 目标 CNode |
| TCB_WriteRegisters | 3 | flags, count, registers… / 无 |
| TCB_Configure | 5 | faultEP, cspaceData, vspaceData, ipcVA / CNode, VSpace, IPCFrame |
| TCB_SetPriority | 7 | priority（0..=255，默认 0）/ 无 |
| TCB_SetIPCBuffer | 9 | ipcVA（0 = 清除）/ IPCBufferFrame（须已映射在该 VSpace 的 ipcVA） |
| TCB_SetSpace | 10 | faultEP, cspaceData, vspaceData / CNode, VSpace |
| TCB_Suspend / Resume | 11 / 12 | 无 |
| CNode_Revoke / Delete | 17 / 18 | index, depth / 无 |
| CNode_Copy | 20 | dstIndex, dstDepth, srcIndex, srcDepth, rights / 源 CNode |
| CNode_Mint | 21 | Copy 参数加 capData / 源 CNode；目前只支持 capData=0 |
| CNode_Move | 22 | dstIndex, dstDepth, srcIndex, srcDepth / 源 CNode |
| ARM_PageTable_Map | 38 | VA, attributes / VSpace |
| ARM_PageTable_Unmap | 39 | 无；要求表中没有页面映射 |
| ARM_Page_Map | 40 | VA, rights, attributes / VSpace |
| ARM_Page_Unmap / GetAddress | 41 / 46 | 无 |
| ARM_ASIDPool_Assign | 48 | 无 / VSpace |

Retype 支持 TCB=1、CNode=4、VSpace=6、SmallPage=7、PageTable=9。CNode sizeBits=16，其他支持的固定大小对象 sizeBits=0；一次最多 32 个对象。目标使用直接根 CNode 形式，nodeIndex=nodeDepth=0。内核先检查目标槽和资源，成功后才发布对象。

CapRights 为 Write=1、Read=2、Grant=4、GrantReply=8；内存映射使用 Cacheable=1、ExecuteNever=4。不要与运行时批量映射的 R=1/W=2/X=4 混淆。Frame 的最终访问权限不得超过该 cap 的权限；复制出的 cap 可以建立独立映射。执行映射维护指令缓存一致性，单个映射拒绝 RWX。

**Call 要求 GrantReply（2026-09 P0.2，明示语义决策）**：endpoint 上的 `Call` 把发送方驻留在回复关系上直到对端 Reply；cap 缺 GrantReply 时该回复能力永远无法形成，调用方将永久滞留。本实现选择 seL4 对齐的显式错误：`Call && !GrantReply` 在 syscall 边界立即返回 `Unsupported`（权限拒绝），不投递、不入队、绝不驻留。Send/NBSend/Recv 不受影响（它们不建立回复关系）。

**优先级调度（2026-09 P1.1）**：调度器为每任务 `priority`（u8，默认 0），就绪队列每次弹出版本取最高优先级、同优先级保持 FIFO 入队顺序（抢占/让出的任务回队尾 = 同级轮转）。`TCB_SetPriority`（label 7，与 seL4 XML 顺序一致）只带一个 `priority` 字：seL4 的独立 authority cap 与 per-thread maxPriority 简化为"被调用 TCB cap 本身即授权"（WRITE 校验在 dispatch）。优先级在每次出队时重估，对运行/就绪/阻塞/睡眠中的任务都即时生效；不做优先级继承，高优先级可饿死低优先级（明确记录的策略）。默认 0 保证未显式设置优先级的系统与原 FIFO 行为完全一致。

TCB_Configure 校验 IPCFrame 确实映射在指定 IPC 地址。多个 TCB 允许配置/绑定同一 CNode 与 VSpace（线程组：组成员共享全部 cap 槽位与映射，`docs/fault-handler.md` §3）；`TCB_SetSpace`/`TCB_SetIPCBuffer` 只作用于未启动线程，其余生命周期语义与 Configure 一致。WriteRegisters 支持初始配置，寄存器顺序遵循 AArch64 seL4 UserContext：pc, sp, spsr, x0..x8, x16..x18, x29, x30, x9..x15, x19..x28, tpidr_el0, tpidrro_el0。非零 TLS 配置暂不支持（IPC buffer 地址由内核按任务在进入用户态时装入 TPIDRRO_EL0，组内各线程各有一份）；内核校验入口、栈及用户 PSTATE，拒绝特权模式。Resume 才使准备完成的任务可运行。

## fatboot 装载普通程序

`#[entry]` 继续封装用户入口，应用不编写 `_start` 或裸 SVC。`projects/libs/user/src/elf.rs` 使用上述标准对象操作装载 hello：

1. 复制一份 Untyped cap 作为本次装载的资源祖先，Retype TCB、CNode、VSpace，并 Assign ASID。
2. Retype PageTable 和 Frame，按 ELF 权限建立目标映射。复制 Frame cap，将副本临时映射到 root 的 scratch VA，填充 ELF 文件内容，再撤销临时映射。
3. 创建栈和 IPC 页，只向子 CSpace 复制自身 TCB/CNode/VSpace/IPCFrame 及 Runtime 服务能力，不授予父 CSpace 或 Untyped。
4. Configure TCB，WriteRegisters 设置 PC/SP，Resume。父任务通过 Runtime Wait 等待完成。
5. 销毁时 Revoke 并 Delete 私有 Untyped cap，回收整组对象和子 CSpace；装载失败也使用该路径回滚。

scratch 地址来自 BootInfo 扩展区之后的空闲页，由调用者独占保留；没有固定 ELF 装载地址。页表层级仍受下述实现范围限制。

### EL0 加载契约(对 C 程序同样适用)

`elf.rs` 对 Rust 与 C 程序一视同仁。C `_start` 必须遵守:

- 入口寄存器:`x0 = SpawnInfo 参数页 VA`(Rust 服务经 `Service::init` 读取;
  C 程序可忽略但不得假设 `x0 = 0`);`SP = loader 提供的 64 KiB 栈顶`。
- 段:`.bss` 由 loader 从零化帧 retype,天然清零;代码段 W^X,数据段 RW/NX。
- 参数页 v2(`./cmd arg…` 时):`SpawnInfo` 之后紧跟可选 `ArgvBlock`
  (`magic/argc/total/字符串`,见
  [interpreter-app.md](interpreter-app.md) 决策 H);无参程序页内没有该块。
  **argv 约定（P2.3）**：argv 就是 `./cmd` 之后的 token 序列，shell 不前插
  程序名——脚本运行类程序里 `argv[0]` 即脚本路径。
  脚本运行类子进程在槽 53 可收到一份 fs 能力(决策 I,可能不存在)。
- 退出/崩溃:与 Rust 服务相同,走 `Call(control_ep, EXIT/READY)` 协议或
  fault 投递;不做裸 `ret`。
- C rt0 模板:`_start: 解析参数页(可选 argv)→ `port_main(argc, argv)` → 永不返回`。

契约细节与"为什么解释器需要它"见
[interpreter-app.md](interpreter-app.md)(决策 C/H/I)。

## 显式运行时扩展

睡眠、时钟、退出状态和受限监督原语由内核 Runtime 对象提供，通过 Call 调用，标签独立保留在 0x1000 以上。它们不是 seL4 标准对象方法，也不是已经实现的用户态服务端。C0–C3（[capability-authority-untyped.md](capability-authority-untyped.md) §7）之后，托管任务服务（Create/Start/Status/Wait/Destroy/DestroyThread/Cspace/Vspace/FindEmptySlot/Map）收进内核 `managed-runtime` 特性门控：生产镜像编译掉，调用返回 `Unsupported`，仅 managed 回归套件构建；其余标签按下列三档保留。

| 标签 | 方法 | 处置 |
| --- | --- | --- |
| 0x1000, 0x1008, 0x1009, 0x1010 | Current, Clock, AvailableFrames, DebugConsoleAvailable | 信息类（只读，无资源/控制效果） |
| 0x1006, 0x1007, 0x1014 | Sleep, Exit, Shutdown | 受限自指原语（只影响调用者；PSCI 仅 EL1 可达） |
| 0x100b..0x100e | Unmap, Protect, WriteMemory, ReadMemory | 受限监督原语（目标须为 WRITE TCB cap 且可编辑：self/停止/fault 阻塞） |
| 0x1001..0x1005, 0x100a, 0x100f..0x1013 中的托管项 | Create, Start, Status, Wait, Destroy, DestroyThread, Cspace, Vspace, FindEmptySlot, Map | `managed-runtime` 门控（生产 `Unsupported`） |

`Shutdown` 执行 PSCI `SYSTEM_OFF`（`hvc #0`，QEMU 平台），不返回；持 Runtime 能力的任务均可调用，shell 的 `exit` 用它。`Unmap` 服务于 root 运行时的栈守卫（boot 映射页没有 frame cap），Write/Read 服务于监督者对可编辑目标的检视与修复；二者均无隐式资源路径。

**Sleep 的裁决（2026-09 P1.2，明示偏离）**：seL4 没有 sleep 系统调用。本实现保留 `Sleep` 为**受限自指原语**（方案 B）：只作用于调用者自身（`Disposition::Sleep` 驻留当前任务至 deadline），经 Runtime cap 校验，无任何跨任务或资源副作用，与 `Exit/Shutdown` 同类，可解释、不破坏 C0–C3 的能力模型。把 Generic Timer 交给用户态 timer 服务（sleep = 向该服务 IPC）仍列为 stretch：单核上内核调度 tick 与用户 timer 争用 PPI，需要拆分 tick 归属，实现与验证成本与当前收益不匹配（[roadmap-next.md](roadmap-next.md) P1.2 方案 A）。

参数封装见 `projects/libs/user/src/task.rs` 和 `kernel/src/object/runtime.rs`。目标参数也是调用者 CSpace 的 TCB cap，每次调用均重新解析。跨任务授权通过复制 cap 完成，不再依据"目标是不是直接子任务"。用户态任务的销毁是标准能力操作：revoke loader 派生子树（`rstiny::Task::destroy`），终止通知走 control endpoint 协议。

## 尚未对齐的部分

- Endpoint/Notification/fault endpoint 已实现（见"调用边界"），但仍与 seL4 有偏离：MessageInfo 不支持 capsUnwrapped/badge 解包（收到的 cap 不带 badge）；wait queue 是 FIFO，没有优先级队列；IPC 无超时/MCS；`Call` 无 GrantReply 立即报错（上述 P0.2 决策）。
- Untyped 目前是真实物理区间 + watermark 切分：`retype` 从父 Untyped 取内存并记录归属，`CNode_Revoke` 作用于 Untyped cap 时 finalise 子对象、清零并重置区间。它仍不是 seL4 的精确对象内存布局与完整 MDB 撤销；TCB/CNode 元数据留在对象表、不计入 Untyped 预算。
- VSpace 内部自动创建 L1/L2；显式 PageTable 对象只对应 L3。ASID Assign 有逻辑约束，但硬件仍使用 ASID 0 和完整 TLB 失效。
- 页表 Unmap 要求为空；没有完整的递归解除映射语义。Revoke 可能在部分解除映射后因非空页表失败，保留的 cap 会同步清除已解除的映射记录，可在处理剩余映射后重试。CSpace 不支持任意深度、badge 或完整 Mint guard 操作。
- TCB Configure/SetSpace/SetIPCBuffer/WriteRegisters 只覆盖未启动线程的初始配置（fault 修复路径对 `TASK_BLOCKED_FAULT` 放开 WriteRegisters）；ReadRegisters 尚未实现。多个 TCB 可共享 CSpace/VSpace（线程组），但没有跨线程 TLS 或 MCS；优先级已实现（上述 P1.1，无继承/预算）；组销毁语义见 `Runtime::Destroy/DestroyThread`（本文档"显式运行时扩展"一节）。
- BootInfo 仍为本项目 128 字节、版本 5 的头部，不是 seL4_BootInfo；v5 发布 `untyped_start`/`untyped_count` 与扩展记录 id=7 的 Untyped 描述列表。错误附加消息与 TLS 约定也尚未完全对齐。

验证入口为 `make check`：ABI wire fixtures、实际 EL0 寄存器调用、对象配置、跨 CSpace cap 授权、权限衰减、撤销回收、资源耗尽回滚，以及 fatboot 的标准对象 ELF 装载均纳入回归。
