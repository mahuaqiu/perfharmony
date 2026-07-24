# Harmony 指标映射

这些 key 是 `Sample.hwinfo_raw` 和 Worker 上报使用的稳定键。单位是当前代码的输出单位；带有“待校准”的指标不能在真机证据冻结前作为准确物理量对外承诺。

| key | 单位 | 语义 | 状态 |
| --- | --- | --- | --- |
| `Harmony CPU Usage` | `%` | 整机 CPU 使用率，范围 `[0,100]` | P0；真实字段待确认 |
| `Harmony CPU User` | `%` | 用户态 CPU | P0；真实字段待确认 |
| `Harmony CPU System` | `%` | 系统态 CPU | P0；真实字段待确认 |
| `Harmony CPU Idle` | `%` | 空闲或 iowait 兼容字段 | P0；真实语义待确认 |
| `Harmony Mem Total` | `MB` | 系统内存总量 | P0；hidumper 字段待确认 |
| `Harmony Mem Used` | `MB` | 系统内存使用量 | P0；used 语义待确认 |
| `Harmony Mem Available` | `MB` | 系统可用内存 | P0；字段待确认 |
| `Harmony Net Upload` | `KB/s` | 选定网卡集合的上行速率 | P1；节点待确认 |
| `Harmony Net Download` | `KB/s` | 选定网卡集合的下行速率 | P1；节点待确认 |
| `Harmony Battery Current Raw` | `raw` | current_now 原始值 | P2；单位待校准 |
| `Harmony Battery Voltage Raw` | `raw` | voltage_now 原始值 | P2；单位待校准 |
| `Harmony Power` | `W` | 电流乘电压的估算功耗 | P2；真机校准后再启用 |
| `Harmony CPU Temp` | `°C` | 当前选中的 thermal zone 温度 | P2；zone 含义待确认 |
| `Harmony GPU Usage` | `%` | GPU 使用率 | P2；命令和字段待确认 |

## 进程内存语义

当前目标进程的 `working_set_mb` 与 `committed_memory_mb` 暂映射为同一解析结果，通常接近 RSS/PSS，不能解释为已经冻结的 VSS 或 Windows committed memory。真机拿到 `hidumper --mem <pid>` 与 `top` 原文后，需要明确 RSS/PSS/VSS 映射，再决定是否拆成独立字段。

## TopN

`top_n_cpu` 和 `top_n_gpu` 当前固定为 `None`。只有在确认单次批量快照能覆盖全量进程、并满足 Worker 单轮 HDC 往返预算后，才允许启用 TopN；之前的 API 字段保留但不代表能力可用。
