# perfharmony

> Implementation status: P0 uses synthetic fixtures/FakeHdc until PC and Mobile real-device command output is frozen. Optional collectors fail closed and never replace unavailable values with zero.

## Capability probe and wheel build

`perfharmony.probe(udid)` reports target status and per-capability `available`, `degraded`, or `unavailable` with a reason. `top_n_cpu` and `top_n_gpu` remain reserved and return `None` until a real-device batch CPU snapshot is confirmed.

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
- P0 真机采集需要先完成 PC/Mobile 命令输出冻结；没有真机时只能运行 fixture 和 FakeHdc 测试。

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
