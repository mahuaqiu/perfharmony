# perfharmony 开发约定

- 所有跨仓协议、设备身份和采样语义以 `docs/command-contract.md` 为准。
- `device_id` 是平台数据库关联 ID；`device_sn` 是 HDC UDID，二者不能混用。
- P0 解析器必须由 PC 和 Mobile 真机 fixture 驱动；合成 fixture 只覆盖边界和失败路径。
- 每个 UDID 的 HDC 命令串行执行，单轮命令预算不能超过契约上限。
- 代码注释使用中文，优先保持 Rust/PyO3 API 与 perfwin 可观察行为一致。

