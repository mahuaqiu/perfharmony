# 鸿蒙真机命令实测报告（HAD-W32 2in1）

> 用途：与团队讨论真机输出与现有解析器的差异及修改方案。
> 采集方式：通过远程 Worker（http://192.168.0.105:8088）的 `cmd_exec` 通道执行宿主机 HDC 命令，
> 脚本：`scripts/capture_real_fixtures.py`，输出原文已冻结到 `tests/fixtures/real/harmony_pc/`。

## 设备信息

| 项 | 值 |
|---|---|
| UDID | 3QC0124A10000066 |
| 型号 | HAD-W32（HUAWEI MateBook Pro） |
| 系统版本 | 6.1.0.130(SP13ENTC293E107R2P1log) |
| API Level | 24 |
| 形态 | `const.product.devicetype` = `2in1`（归 harmony_pc） |
| 连接 | USB，`hdc list targets -v` 状态列为 `Connected` |

## 一、执行的命令与结果

| # | 命令 | 结果 | 冻结文件 |
|---|---|---|---|
| 1 | `hdc list targets -v` | `3QC0124A10000066\t\tUSB\tConnected\tlocalhost\thdc`（tab 分隔，状态列 Connected 而非 Ready） | list_targets_v.txt |
| 2 | `param get const.product.model / software.version / apiversion` | HAD-W32 / 6.1.0.130(...) / 24 | device_info.txt |
| 3 | `param get const.product.devicetype` | `2in1`（形态分类唯一可用键，type/device_type/form/family 均 errNum 106） | device_info.txt |
| 4 | `hidumper -h` | 正常，`--cpuusage`/`--mem` 均支持 | hidumper_help.txt |
| 5 | `hidumper --cpuusage` | 正常输出，约 22 KB，单行分号汇总 + 进程明细表 | hidumper_cpuusage.txt |
| 6 | `hidumper --mem` | 正常输出，约 74 KB，按 PID 的 PSS 表 + 尾部 Total/Free/Used RAM 汇总 | hidumper_mem.txt |
| 7 | `hidumper --mem <pid>` | 正常输出，三行表头的列表格（Pss Total/Shared Clean/...） | hidumper_mem_pid.txt |
| 8 | `ps -ef` | 正常，标准 8 列（UID PID PPID C STIME TTY TIME CMD），189 行 | ps_ef.txt |
| 9 | `cat /proc/stat`（间隔 2s 两次） | 标准格式，cpu 行 10 个数值字段 | proc_stat_t0/t1.txt |
| 10 | `cat /proc/meminfo` | 标准格式，MemTotal/MemFree/MemAvailable 全有 | proc_meminfo.txt |
| 11 | `cat /proc/net/dev` | 标准格式，wlan0 存在且有流量 | proc_net_dev.txt |
| 12 | `which hiperf; which top` | `/bin/hiperf`、`/bin/top` 均存在 | which_tools.txt |
| 13 | `top -n 1 -b` | 正常，11 列：`PID USER PR NI VIRT RES SHR S %CPU %MEM TIME+ ARGS` | top_n1_b.txt |
| 14 | `hidumper -s 10 -a screen` | 正常，`render resolution=3120x2080`、`powerStatus=POWER_STATUS_ON` | Worker 远程复测，待补独立 fixture |

## 二、与现有解析器的兼容性结论

### ✅ 兼容、无需修改（6 项）

| 命令 | 解析器 | 结论 |
|---|---|---|
| `hdc list targets -v` | `parse_list_targets`（client.rs） | 上轮 Connected 修复已生效 |
| `cat /proc/stat` | `parse_proc_stat` | 兼容 |
| `cat /proc/meminfo` | `parse_proc_meminfo` | 兼容 |
| `cat /proc/net/dev` | `parse_proc_net_dev` | 兼容（wlan0 计数 6.1 GB，u64 无溢出） |
| `ps -ef` | `parse_ps_ef` | 兼容，包名/可执行名提取正常 |
| `top -n 1 -b` | `parse_top_processes`（monitor.rs） | 兼容，fields[8]=%CPU、fields[5]=RES(如 128M) 对上 |

### ✅ 已修复 1：`hidumper --cpuusage` 字段丢失 + idle 值错误

真机汇总是**单行分号格式**（合成 fixture 猜的是多行格式）：

```
Total: 2.83%; User Space: 1.27%; Kernel Space: 1.55%; iowait: 0.01%; irq: 0.02%; idle: 97.15%
```

`src/parse/hidumper_cpu.rs` 三处不匹配：

1. `Total:` 能匹配 → cpu_percent 正确，采集不报错（问题被掩盖）；
2. 正则 `User\s+space` / `Kernel\s+space` 是小写 s，真机是 `User Space`/`Kernel Space`
   → user/system 两个传感器**静默丢失**；
3. idle 正则 `(?i)(Idle|iowait)` 在文本中先命中 `iowait: 0.01%`
   → "Harmony CPU Idle" 上报 0.01%（其实是 iowait），真实 idle 97.15% 被顶掉。

### ✅ 已修复 2：`hidumper --mem` 系统内存解析失败

真机输出没有 `Total Memory:` 行，汇总在尾部且格式不同：

```
      Total RAM:32680648 kB
       Free RAM:24154539 kB (23034579 cached + 1119960 free)
       Used RAM:5483865 kB (3790805 total pss + 1693060 kernel)
```

`parse_hidumper_mem_system` 找的是 `Total Memory:/Used Memory:/Free Memory:`，100% 失败。后果：

- monitor 靠 fallback 链兜底（对同一文本再试 meminfo 解析、失败后**额外发一次** `cat /proc/meminfo`）
  → 数据不缺，但每轮采样白多一次 shell 往返；
- `probe()` 会把 `system_memory` 能力报成 **degraded**，误导排障。

### ✅ 已修复 3：`hidumper --mem <pid>` 三行表头格式

真机是三行表头的列表格：

```
                          Pss         Shared         Shared        Private ...
                        Total          Clean          Dirty          Clean ...
                       ( kB )         ( kB )         ( kB )         ( kB ) ...
            GL              0              0              0              0 ...
AnonPage other         207406           6352              0         204548 ...
         Total         207422           6352              0         204564 ...
```

`parse_hidumper_mem_pid` 的 `Pss\s*\(\s*kB\s*\)` 表头检测直接失败（Pss 与 ( kB ) 不在同一行）；
且 Total 行没有 "Pss" 字样，Total 提取分支也走不到。
目前该函数在 monitor/lib 中未被实际调用（进程内存走 top 的 RES 列），影响小，但属于库对外契约，建议一并修。

## 三、修改结果

1. **hidumper_cpu.rs**：已改为大小写不敏感；`idle` 不再与 `iowait` 共用正则，真机值验证为 97.15%；
2. **hidumper_mem.rs**：
   - `parse_hidumper_mem_system` 已兼容真机 `Total RAM:/Used RAM:/Free RAM:`，保留旧格式兼容；
   - `parse_hidumper_mem_pid` 已按行读取「Total 行首个数字 = Pss Total」，兼容三行表头；
3. **真机契约测试**：Rust 侧已读取 CPU、内存、进程、网络、`ps` 和 `top` 真机 fixture，`cargo test` 43 项通过；
4. **批量进程内存已接入**：复用 `hidumper --mem` 全量表，`working_set_mb` 使用 RSS，`committed_memory_mb` 使用 VSS；CPU 或内存缺失的进程不写入；
5. **TopN CPU 已接入**：复用 `ps -ef`、`top -n 1 -b` 和同一份内存表，按同名聚合、CPU 降序截取，正常路径保持 4 次 HDC 命令；
6. **待部署复测**：重建 wheel、重新打包 Worker，在 105 上执行 `probe()`、目标进程采样、TopN 和持续采样。

Worker 屏幕信息复测确认：`hidumper -s 10` 在该设备只返回 RenderService 帮助，正确命令是
`hidumper -s 10 -a screen`。Worker 已改用该 action，并从 `render resolution` 与
`powerStatus` 解析分辨率和屏幕状态。

> 上述新增能力仅由 Harmony PC 真机证据支持；Harmony Mobile 仍不能据此标记完成。

## 四、覆盖安装注意事项

Windows 覆盖安装前必须先停止 Worker，并终止**可执行文件路径位于当前安装目录**
`tools\hdc\hdc.exe` 的 HDC 进程。HDC 进程持有该目录下的可执行文件或依赖 DLL 时，
安装程序无法覆盖 `tools\hdc`，会导致安装失败。

清理逻辑应按进程可执行路径精确匹配当前安装目录，不应直接使用
`taskkill /IM hdc.exe` 杀掉机器上其他 Harmony SDK、开发工具或测试环境启动的 HDC。
安装完成后由 Worker 正常启动并重新拉起内置 HDC。

## 附：worker（autotest）侧结论

上轮两个修复经真机验证有效：

- `HdcTarget.is_ready` 接受 `Connected` 状态 → 设备发现正常；
- 形态分类增加 `const.product.devicetype` 键 → 该设备正确归类 harmony_pc。

### ⚠️ 新发现（已修复）：`shell()` 手工双引号包裹导致设备不入池

部署 0.1.1 包后现场日志仍报「鸿蒙设备形态未知，暂不加入可执行池」+
「未能解析屏幕分辨率」，平台选不到设备。通过 105 Worker 对照实验定位根因：

- worker 的 `HarmonyHdcWrapper.shell()` 会把命令手工包裹双引号后经
  `subprocess`（list 传参，shell=False）交给 hdc.exe；Windows `list2cmdline`
  会再转义一层，导致 **literal 双引号透传到设备端**；
- 设备端 `/bin/sh` 把整段字符串当成单个命令名：
  `/bin/sh: param get const.product.devicetype: inaccessible or not found`；
- 该错误输出 exit_code=0 且不含 fail 字样，`device_category()` 无法识别，
  所有 `param get`/`hidumper` 链路静默失败 → 形态 unknown → 不入池；
- 此前手工验证都走 cmd_exec → cmd.exe，外层引号被 C runtime 消费，
  hdc 收到干净命令，所以命令本身验证全部通过——两种通道的引号行为不同。

对照实验（`autotest/scripts/probe_remote_hdc.py`）：同一条命令，干净单参数形态
返回 `2in1` / `render resolution=3120x2080`，内嵌引号形态（模拟 worker）返回
`inaccessible or not found`。

修复：`shell()` 不再手工包裹，裸命令直接作为单个 list 参数传递（list2cmdline
自动加的引号会被 hdc.exe 消费，不会泄漏到设备端）；兼容旧调用方自带的包裹
引号（剥离）。回归测试 `test_shell_passes_bare_command_without_wrapping_quotes`，
`tests/test_harmony_platform.py` 39 项全部通过。**需重新打包部署 Worker 生效。**

perfharmony Rust 侧无此问题（`client.rs` 直接 `["-t", udid, "shell", command]`
传参，无手工包裹）。

### 按进程名采集改为按应用汇总（0.1.3）

现场反馈：按进程名采集的结果和按 PID 采集一样，没有像鸿蒙任务管理器那样
按应用汇总。真机 ps -ef 实证：一个应用是多进程，子进程以 `包名:子进程名`
命名（appspawn 约定），如 `com.huawei.his.xgate` + `:gpu` + `:render`×2、
`com.hisec.endpoint` + `:EDRMaster0/:EDRManager1/:EDRScanner2`。

旧行为的两个缺陷：

- `ProcessFilter::Name/Names` 全名精确匹配 → 只命中主进程，子进程全漏，
  “按名”退化成“按单 PID”；
- `aggregate_processes` 按完整进程名归组 → `:render` 和主进程永远分在
  不同组，不会合成一行。

开源依据：SP_daemon（developtools smartperf_device）按包名解析出主进程 +
全部子进程 pid 一起采集（CPU.cpp `GetSysProcessCpuLoad` 的 ChildProc 系列、
 RAM.cpp 的 childPss 系列），任务管理器同样按应用汇总。

修复（monitor.rs）：

- `name_matches()`：按名过滤同时命中 `包名` 和 `包名:子进程`，不误伤
  同前缀包（xgate 不会命中 xgate.esm）；
- `bundle_base_name()`：汇总归组键取冒号前包名（需含 `.` 且不含 `/`，
  内核线程 kworker/0:1H 不归组），aggregated/top_n_cpu 一行代表一个应用；
- 平台后端 `convert_aggregated_to_target_processes` 改用 `pids` 关联实例
  （子进程名≠汇总行名，按名关联会丢实例），实例携带自身进程名供前端
  展示子进程明细。

回归测试 `name_filter_matches_bundle_child_processes`、
`aggregation_groups_children_under_bundle_name`、`bundle_base_name_keeps_non_bundle_names`，
cargo test 46 项全部通过。版本 0.1.2 → 0.1.3。**需重新打包部署 Worker 生效。**
