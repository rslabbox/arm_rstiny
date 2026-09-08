# 来源与本地修改

复制自 `/root/codes/x-kernel/drivers/platform/rs-fdtree`，参考工作树 HEAD 为 `5f843c7e5121c41cc5f562605061928c00e77fe9`。保留全部源码、测试及 DTB/DTS 样本，并附上源项目的 Apache-2.0 LICENSE 和 NOTICE。

本地修改：Cargo 元数据改为独立声明；纠正 FDT_END 为 9；构造时校验版本、块范围、保留表、结构嵌套、属性边界与字符串，限制深度为既有父节点栈支持的 63 层。新增 BadStructure 错误和畸形输入回归。原测试的结束标记同步修正，格式适配本项目工具链。

该副本支持 FDT v17 及向后兼容 v17 的版本，不依赖 alloc。fatboot 在用户态用它解析扩展 BootInfo 提供的 DTB；内核不依赖此库。可用的内存和保留区 API 尚未接入动态资源分配。
