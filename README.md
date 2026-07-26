# perfharmony

> 实现状态：Harmony PC 的系统 CPU、系统内存、进程 CPU/内存和 TopN CPU 已有真机 fixture；Harmony Mobile 仍待真机冻结。可选指标采集失败时保持缺失，不以 0 冒充数据。

## Capability probe and wheel build

`perfharmony.probe(udid)` 会按能力返回 `available`、`degraded` 或 `unavailable` 及原因。Harmony PC 已确认 `top -n 1 -b` 可提供批量 CPU 快照，`top_n_cpu` 已实现；`top_n_gpu` 仍保持 `None`。

```powershell
python -m venv .venv
.\.venv\Scripts\Activate.ps1
python -m pip install maturin pytest
python -m maturin build --release
python -m pytest -q
```

See [`docs/command-contract.md`](docs/command-contract.md) for shell commands and real-device gates, and [`docs/metric-mapping.md`](docs/metric-mapping.md) for stable Harmony keys.

通过 HDC 从 HarmonyOS PC/移动设备采集性能数据的 Rust + PyO3 库。

当前实现阶段：

- 已提供可编译的 PyO3 包骨架。
- 已提供 HDC 路径解析、目标列表解析和 shell 客户端。
- Harmony PC P0 已按真机输出实现并有 fixture 契约测试；Harmony Mobile 仍只能视为待验证。

开发环境：

```powershell
python -m venv .venv
.\.venv\Scripts\Activate.ps1
python -c "import sys; assert sys.prefix != sys.base_prefix"
python -m pip install maturin pytest
python -m maturin develop
python -m pytest -q
```

路径优先级：显式 `hdc_path`、`PERFHARMONY_HDC`、`HDC_PATH`、Worker 注入的包内路径、SDK 环境变量、PATH。

## 身份约定

平台数据库的 `device_id` 只用于任务和上报关联。鸿蒙设备的 `device_sn` 必须是 HDC target UDID，并作为 `Monitor(udid=...)` 的参数。
