# P0 真机命令输出归档目录

本目录存放从真实鸿蒙设备采集的原始命令输出，用于驱动解析器单元测试。

## 状态

⏳ 等待真机接入（参见 ISSUE-001）

## 目录规划

```
real/
  harmony_pc/
    device_info.txt          # param get const.product.type 等设备指纹
    hidumper_cpuusage.txt    # hidumper --cpuusage
    hidumper_mem.txt         # hidumper --mem
    hidumper_mem_pid.txt     # hidumper --mem <pid>
    proc_stat_t0.txt         # cat /proc/stat (第一次快照)
    proc_stat_t1.txt         # cat /proc/stat (第二次快照，差分用)
    proc_meminfo.txt         # cat /proc/meminfo
    proc_net_dev.txt         # cat /proc/net/dev
    ps_ef.txt                # ps -ef
    list_targets_v.txt       # hdc list targets -v (真机)
  harmony_mobile/
    (同上，Mobile 设备各一份)
```

## 归档规范

- 每份 fixture 第一行注释设备指纹：`# device: <型号> <系统版本> <API Level>`
- 保留命令输出原文，不做任何修改
- 合成 fixture 只允许存放在 `tests/fixtures/` 根目录，不允许混入本目录
