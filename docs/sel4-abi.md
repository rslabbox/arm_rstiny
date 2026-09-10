# seL4 风格 ABI 与内核对象

当前实现对齐 seL4 **AArch64、non-MCS、单核、无硬件调试/SMMU/VCPU** 配置的调用协议及部分对象方法，不是完整的 seL4 二进制兼容实现。参考本地 `../seL4/kernel`，提交 `28b8f4c40d4a48a206bedc7875c6695f6106f34b`。

协议依据为 `libsel4/include/api/syscall.xml`、`libsel4/include/interfaces/object-api.xml`、ARM/AArch64 的 `object-api-arch.xml`、`object-api-sel4-arch.xml`，以及 AArch64 `syscalls.h` 和 64 位 `shared_types.bf`。对象调用标签由 XML 顺序和配置共同决定，不能把本表应用于任意 seL4 构建。

## 调用边界

用户通过 `svc #0` 陷入，x7 为有符号系统调用号：

| 调用 | 号 | 当前行为 |
| --- | --- | --- |
| Call | -1 | 调用 x0 capability 指向的内核对象 |
| ReplyRecv | -2 | 尚未实现，调用任务故障 |
| Send / NBSend | -3 / -4 | 尚未实现，调用任务故障 |
| Recv / Reply | -5 / -6 | 尚未实现，调用任务故障 |
| Yield | -7 | 让出 CPU，保留用户通用寄存器 |
| NBRecv | -8 | 尚未实现，调用任务故障 |
| DebugPutChar | -9 | 输出 x0 低字节；LOG=off 时不可用 |

Call 输入为 x0=CPtr、x1=MessageInfo、x2..x5=前四个消息字。更多消息字从当前任务登记的 IPC buffer 读取；额外 capability 也从该 buffer 读取。CPtr 总是在调用者的 CSpace 中解析，不是全局任务 ID。

MessageInfo 的 bit 0..6 为 length、7..8 为 extraCaps、9..11 为 capsUnwrapped、12..63 为 label；最多 120 个消息字、3 个额外 capability。请求不接受 capsUnwrapped。返回 x0=0，x1 为回复标签及长度，返回值放在 x2 开始的消息寄存器中；x6..x30 保留。当前对象方法只返回零或一个消息字。

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

TCB_Configure 校验 IPCFrame 确实映射在指定 IPC 地址。多个 TCB 允许配置/绑定同一 CNode 与 VSpace（线程组：组成员共享全部 cap 槽位与映射，`docs/fault-handler.md` §3）；`TCB_SetSpace`/`TCB_SetIPCBuffer` 只作用于未启动线程，其余生命周期语义与 Configure 一致。WriteRegisters 支持初始配置，寄存器顺序遵循 AArch64 seL4 UserContext：pc, sp, spsr, x0..x8, x16..x18, x29, x30, x9..x15, x19..x28, tpidr_el0, tpidrro_el0。非零 TLS 配置暂不支持（IPC buffer 地址由内核按任务在进入用户态时装入 TPIDRRO_EL0，组内各线程各有一份）；内核校验入口、栈及用户 PSTATE，拒绝特权模式。Resume 才使准备完成的任务可运行。

## fatboot 装载普通程序

`#[entry]` 继续封装用户入口，应用不编写 `_start` 或裸 SVC。`projects/libs/user/src/elf.rs` 使用上述标准对象操作装载 hello：

1. 复制一份 Untyped cap 作为本次装载的资源祖先，Retype TCB、CNode、VSpace，并 Assign ASID。
2. Retype PageTable 和 Frame，按 ELF 权限建立目标映射。复制 Frame cap，将副本临时映射到 root 的 scratch VA，填充 ELF 文件内容，再撤销临时映射。
3. 创建栈和 IPC 页，只向子 CSpace 复制自身 TCB/CNode/VSpace/IPCFrame 及 Runtime 服务能力，不授予父 CSpace 或 Untyped。
4. Configure TCB，WriteRegisters 设置 PC/SP，Resume。父任务通过 Runtime Wait 等待完成。
5. 销毁时 Revoke 并 Delete 私有 Untyped cap，回收整组对象和子 CSpace；装载失败也使用该路径回滚。

scratch 地址来自 BootInfo 扩展区之后的空闲页，由调用者独占保留；没有固定 ELF 装载地址。页表层级仍受下述实现范围限制。

## 显式运行时扩展

睡眠、时钟、退出状态和托管任务便利接口仍由内核 Runtime 对象提供，通过 Call 调用，标签独立保留在 0x1000 以上。它们不是 seL4 标准对象方法，也不是已经实现的用户态服务端。

| 标签 | 方法 |
| --- | --- |
| 0x1000..0x1005 | Current, Create, Start, Status, Destroy, Wait |
| 0x1006..0x1009 | Sleep, Exit, Clock, AvailableFrames |
| 0x100a..0x100e | Map, Unmap, Protect, WriteMemory, ReadMemory |
| 0x100f..0x1013 | FindEmptySlot, DebugConsoleAvailable, Cspace, Vspace, DestroyThread |

`Destroy` 是组级语义：句柄命名一个进程（共享 CSpace 的线程组），先停止全部成员线程再回收对象（[进程/线程组生命周期与组内故障监督](thread-group.md) §2.2）；`DestroyThread` 只销毁单个线程，共享 CSpace/VSpace 留给兄弟线程。

参数封装见 `projects/libs/user/src/task.rs` 和 `kernel/src/object/runtime.rs`。目标参数也是调用者 CSpace 的 TCB cap，每次调用均重新解析。跨任务授权通过复制 cap 完成，不再依据“目标是不是直接子任务”。托管 Create 仍自动建立默认空间及 IPC 页；托管 Destroy 会收回对应托管 CSpace。标准对象创建的任务退出后，空间对象保留到 capability 生命周期结束。

## 尚未对齐的部分

- Endpoint、Notification、用户 IPC 和 fault endpoint 投递未实现；保留调用号不表示具备相应服务。
- Untyped 目前是真实物理区间 + watermark 切分：`retype` 从父 Untyped 取内存并记录归属，`CNode_Revoke` 作用于 Untyped cap 时 finalise 子对象、清零并重置区间。它仍不是 seL4 的精确对象内存布局与完整 MDB 撤销；TCB/CNode 元数据留在对象表、不计入 Untyped 预算。
- VSpace 内部自动创建 L1/L2；显式 PageTable 对象只对应 L3。ASID Assign 有逻辑约束，但硬件仍使用 ASID 0 和完整 TLB 失效。
- 页表 Unmap 要求为空；没有完整的递归解除映射语义。Revoke 可能在部分解除映射后因非空页表失败，保留的 cap 会同步清除已解除的映射记录，可在处理剩余映射后重试。CSpace 不支持任意深度、badge 或完整 Mint guard 操作。
- TCB Configure/SetSpace/SetIPCBuffer/WriteRegisters 只覆盖未启动线程的初始配置（fault 修复路径对 `TASK_BLOCKED_FAULT` 放开 WriteRegisters）；ReadRegisters 尚未实现。多个 TCB 可共享 CSpace/VSpace（线程组），但没有跨线程 TLS、优先级或 MCS；组销毁语义见 `Runtime::Destroy/DestroyThread`（本文档"显式运行时扩展"一节）。
- BootInfo 仍为本项目 128 字节、版本 5 的头部，不是 seL4_BootInfo；v5 发布 `untyped_start`/`untyped_count` 与扩展记录 id=7 的 Untyped 描述列表。错误附加消息与 TLS 约定也尚未完全对齐。

验证入口为 `make check`：ABI wire fixtures、实际 EL0 寄存器调用、对象配置、跨 CSpace cap 授权、权限衰减、撤销回收、资源耗尽回滚，以及 fatboot 的标准对象 ELF 装载均纳入回归。
