# Harmony 性能命令契约

本文档记录 `perfharmony` 当前实现的命令边界。`tests/fixtures/` 中没有 `real/` 设备输出时，只能证明解析器和错误路径，不代表 PC 或 Mobile 真机已经完成适配。

## HDC 目标

| 用途 | 命令 | 结果 | 当前状态 |
| --- | --- | --- | --- |
| 目标发现 | `hdc list targets -v` | UDID、状态、连接类型 | 已实现；需要真机确认表头变体 |
| 单次 shell | `hdc -t <udid> shell <command>` | stdout/stderr | 已实现；15 秒默认超时，可取消并回收子进程 |

HDC 路径优先级为显式 `hdc_path`、`PERFHARMONY_HDC`、`HDC_PATH`、Worker 注入路径、SDK 环境变量、最后才是 `PATH`。同一 UDID 的命令通过进程内锁串行执行。

调试辅助 `perfharmony.list_sensors(udid)` 会按当前已实现的解析器返回稳定 key 列表；它不
替代 `probe()`，也不代表字段语义已经通过 PC/Mobile 真机评审。

## P0 命令

| 指标 | 首选命令 | 回退命令 | 解析器 | 当前状态 |
| --- | --- | --- | --- | --- |
| 系统 CPU | `hidumper --cpuusage` | `cat /proc/stat` 两拍差分 | `parse_hidumper_cpuusage` / `parse_proc_stat` | PC 已确认；Mobile 待确认 |
| 系统内存 | `hidumper --mem` | `cat /proc/meminfo` | `parse_hidumper_mem_system` / `parse_proc_meminfo` | PC 已确认；Mobile 待确认 |
| 进程列表/CPU | `ps -ef` + `top -n 1 -b` | 无 | `parse_ps_ef` + top 行解析 | PC 已确认批量全量快照；Mobile 待确认 |
| 进程内存 | 与系统内存复用同一次 `hidumper --mem` | 无 | `parse_hidumper_mem_processes` | PC 已确认 PSS/VSS/RSS/USS 表；Mobile 待确认 |

Harmony PC 目标进程映射为 `working_set_mb = RSS / 1024`、`committed_memory_mb = VSS / 1024`。进程 CPU 或内存任一缺失时，该进程不进入本轮样本。`top_n_cpu` 复用同一轮 `ps`、`top` 和内存表，按同名进程聚合后按 CPU 降序截取，不增加 HDC 往返；正常 P0 路径仍为 4 次命令。

P0 失败不写入 0。连续三轮没有任何 P0 指标时 Monitor 停止，并通过 `last_error` 暴露原因。

## 可选命令

| 能力 | 命令 | 当前处理 | 真机门禁 |
| --- | --- | --- | --- |
| 网络 | `cat /proc/net/dev` | 两拍计数器差分，写 `Harmony Net Upload/Download`，首拍为 0 | 网卡节点、权限、单位需确认 |
| 功耗 | 读取 `Battery/current_now`、`Battery/voltage_now`，同时尝试小写目录 | 只保存 `Harmony Battery * Raw`，不计算 W | 电流/电压单位和设备节点需校准 |
| 温度 | 读取 `/sys/class/thermal/thermal_zone*/temp` | 暂按 millidegree 转换为 `Harmony CPU Temp` | 节点含义和单位需确认 |
| GPU | `hidumper --gpu` | 仅接受明确的百分号字段，否则不可用 | 命令、字段和权限需确认 |

可选命令在 Monitor 中共用一次 shell；任一段失败不影响已经取得的 P0 样本。

## 真机验收清单

PC 和 Mobile 各需归档以下原始输出，并放入 `tests/fixtures/real/<device-kind>/`。当前
`harmony_pc/` 已有 HAD-W32 真机输出，`harmony_mobile/` 仍待补齐：

```text
hdc list targets -v
param get const.product.type
hidumper -h
hidumper --cpuusage
hidumper --mem
hidumper --mem <pid>
ps -ef
top -n 1 -b
cat /proc/stat
cat /proc/meminfo
cat /proc/net/dev
hidumper --gpu
for p in /sys/class/power_supply/Battery/current_now /sys/class/power_supply/Battery/voltage_now; do cat "$p"; done
for p in /sys/class/thermal/thermal_zone*/temp; do cat "$p"; done
```

还需要目标应用运行前后两次 CPU 快照，并记录系统版本、设备类型和 HDC 版本。真机到位后只更新 fixture、解析器和映射，不改变 `Monitor`、Worker 或 ZQ 的身份契约。

## SP_daemon 采集契约（0.2.0 起，0.3.0 增补）

0.2.0 起设备侧唯一数据源为官方性能守护 `SP_daemon`，长跑流式命令：

```text
SP_daemon -N <拍数> -c -g -f -t -p -r -net -d [-PKG <包名> | -PID <pid>]
```

真机（2LQ0224125000197）实证的行为约束：

- 设备端命令**禁止管道**：SP_daemon 检测到 stdout 非终端会静默退出（exit 0），过滤解析全部在宿主机侧完成。
- 一次采集只能带**一个** `-PKG` **或**一个 `-PID`，重复传参无输出，逗号分隔会把整串当包名。
- `-PKG` 在设备端是**前缀匹配**：`-PKG com.a.test` 会把 `com.a.test.abc` 的进程一并采集进主/子进程列表。宿主侧在模糊模式下按 `ps -ef` 的 pid→进程名映射做精确过滤兜底（`bundle_base_name` 全等比较，映射缺失的新进程保留）。
- `-PID` 为官方参数（真机验证可用），单进程精准采集，不聚合子进程（`ChildProcId=NA`）。
- fuzzy 模式下 Worker 每 10s 执行一次 `ps -ef` 检查目标包名的进程集合；新增、退出或 PID 变化时停止旧流并重启 `SP_daemon`，保留重启前尚未上报的样本。进程集合排除 `SP_daemon` 本身；解析器优先读取可执行文件名，避免把 `-PKG` 参数误判成应用进程。
- Monitor 仍每 30s 刷新 `ps -ef` 的 pid→进程名映射，用于补全子进程明细名称和采样块精确过滤（15s 超时）；子 PID 有映射时展示设备真实名称，无映射时保留“包名:PID”占位名；流式采集不持 target 锁，短命令可并发，属于契约内的命令预算。
- `SP_daemon -stop` 是设备级清理：采集启动前后各执行一次。
- **SP_daemon 为设备级硬单例（真机实证）**：同一设备同一时刻只能存在一个采集流，
  新实例启动会把旧实例顶掉（旧流静默中断，无错误输出）。因此「多应用同时采集」
  不可行，平台侧已按单应用约束实现。Monitor 提供 `auto_cleanup=False` 与模块级
  `stop_daemon(udid)`，供未来需要协调多实例清理的特殊场景使用（当前 worker 不使用，
  单实例保持默认 auto_cleanup=True）。
- 进程内存口径为**常驻内存** `pss − swapPss`（KB→MB，`swapPss` 缺失回退 `pss`），与任务管理器的"真驻留物理内存"口径对齐；主进程细分 `nativeHeapPss/graphicPss/arktsHeapPss/swapPss` 以 `Harmony * Pss`（MB）写入 `hwinfo_raw` 供对数。

## Worker 覆盖安装门禁

覆盖安装 Worker 前先停止 Worker，并终止可执行路径位于当前安装目录
`tools\hdc\hdc.exe` 的进程，否则 Windows 文件锁可能阻止安装程序覆盖 HDC 及其依赖文件。
安装清理必须按完整可执行路径匹配，不能按进程名全局结束所有 `hdc.exe`。
