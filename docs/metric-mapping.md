# Harmony 指标映射

这些 key 是 `Sample.hwinfo_raw` 和 Worker 上报使用的稳定键。单位是当前代码的输出单位；带有“待校准”的指标不能在真机证据冻结前作为准确物理量对外承诺。

| key | 单位 | 语义 | 状态 |
| --- | --- | --- | --- |
| `Harmony CPU Usage` | `%` | 整机 CPU 使用率，范围 `[0,100]` | P0；PC 已确认，Mobile 待确认 |
| `Harmony CPU User` | `%` | 用户态 CPU | P0；PC `User Space` 已确认 |
| `Harmony CPU System` | `%` | 系统态 CPU | P0；PC `Kernel Space` 已确认 |
| `Harmony CPU Idle` | `%` | 空闲 CPU，不含 iowait | P0；PC 已确认 |
| `Harmony Mem Total` | `MB` | 系统内存总量 | P0；PC `Total RAM` 已确认 |
| `Harmony Mem Used` | `MB` | 系统内存使用量 | P0；PC `Used RAM` 已确认 |
| `Harmony Mem Available` | `MB` | 系统可用内存 | P0；PC 无该字段时保持缺失 |
| `Harmony Net Upload` | `KB/s` | 选定网卡集合的上行速率 | P1；节点待确认 |
| `Harmony Net Download` | `KB/s` | 选定网卡集合的下行速率 | P1；节点待确认 |
| `Harmony Battery Current Raw` | `raw` | current_now 原始值 | P2；单位待校准 |
| `Harmony Battery Voltage Raw` | `raw` | voltage_now 原始值 | P2；单位待校准 |
| `Harmony Power` | `W` | 电流乘电压的估算功耗 | P2；真机校准后再启用 |
| `Harmony CPU Temp` | `°C` | 当前选中的 thermal zone 温度 | P2；zone 含义待确认 |
| `Harmony GPU Usage` | `%` | GPU 使用率 | P2；命令和字段待确认 |

## 进程内存语义

Harmony PC 的 `hidumper --mem` 一次返回全量 PID 的 PSS/VSS/RSS/USS 表。当前映射为：

- `working_set_mb = Total Rss / 1024`
- `committed_memory_mb = Total Vss / 1024`

这些字段与 Windows 工作集/提交内存只保持结构同构，不代表跨平台绝对值可直接比较。Mobile 表格格式和语义仍待真机确认；CPU 或内存不完整的进程不会写入该轮样本。

## TopN

Harmony PC 已确认 `top -n 1 -b` 单次返回全量进程 CPU，`top_n_cpu` 已实现：按同名进程聚合，CPU 归一化到 `[0,100]`，降序截取 N。未启用、快照不可用或没有 CPU/内存完整的进程时返回 `None`。Mobile 仍应由调用方关闭该能力；`top_n_gpu` 继续固定为 `None`。
