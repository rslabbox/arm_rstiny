# AGENT.md — 面向 Agent 的工程约束

给在本仓库工作的编码 Agent 的硬性要求。改动前先读，提交前自查。

## 1. 文件大小：main.rs 是薄封装，逻辑进模块

- **`main.rs` 只做三件事**：入口（`#[entry]` / 资源装配）、协议/命令**分发**、
  把子模块的函数粘起来。业务逻辑、数据结构、后端实现一律放独立模块。
- **行数上限**：`main.rs` ≤ **300 行**；其他单文件 ≤ **400 行**。超过即拆，
  没有例外；如果确实拆不动，在 PR/提交说明里写明原因。
- **拆分粒度按职责，不按行数凑**。参照现有范例：
  - `fs-server`：`main.rs`（fs v2 协议循环）+ `backends.rs`（FileSystem
    trait 与 FAT/ext4 后端）+ `blockdev.rs`（块 IPC 适配器）
  - `gpu-server`：`main.rs`（LEASE/FLUSH 协议循环）+ `hal.rs`（DMA 账本与
    virtio Hal）+ `devices.rs`（探测与输入）
  - `init`：`main.rs`（监督循环）+ `state.rs` + `spawn.rs` + `policy.rs` +
    `logger.rs`
- **纯移动优先**：拆分是移动代码，不是重写。改函数可见性（`pub`）、加
  `use`、调整 import，其余保持逐字节一致；拆完必须构建 + 实机冒烟。
- 跨模块共享的常量与类型定义在使用方集中处（如某任务的 `hal.rs` /
  `state.rs`）并 `pub`，避免两份定义。
- **存量超限**（渐进重构的已知债务，动到哪个文件就顺手拆到达标）：
  `gpu-server/main.rs`、`mysh/main.rs`、`init/main.rs`、`gui/wm.rs`、
  `fs-server/main.rs`、`block-server/main.rs`、`appmgr/main.rs`。

## 2. 不留诊断脚手架

- 调试用的探针、`[xxx][trace]` 日志、`log::warn!` 打点、临时的
  `sleep()` settle delay，**问题定位完必须随修复一起删除**。保留的只有
  正常错误路径日志（如 "mount failed"）。
- 批量替换脚本时每处替换必须断言生效（`replace` 不匹配是静默 no-op，
  曾导致宏"已更新"的假象）。

## 3. 验证门禁

- **`make check` 是唯一回归门禁**，落地任何内核/用户态改动后必须全量
  跑通再提交。单测过的组合不等于全组合（debug/release × LOG=off/info）。
- `make check` 运行期间**不得编辑任何源码**——cargo 会把中间态编进
  后续检查，产生与改动无关的假失败。改代码等 check 结束。
- 后台跑 `make check` 时同样禁止并行构建或起停 QEMU。

## 4. 提交

- 每个提交自我完整：编译通过、`make check` 通过（或明确注明待验证项）。
- 提交说明写清楚：动机、根因或设计取舍、验证方式。
- 经由 HAPI 的会话按惯例在提交说明末尾附 HAPI 署名。
