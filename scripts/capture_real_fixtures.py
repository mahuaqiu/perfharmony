# -*- coding: utf-8 -*-
"""P0 真机 fixture 冻结脚本：通过远程 Worker 的 cmd_exec 通道抓取鸿蒙真机命令输出。

对应设计规范 §2.11：把真机命令输出原文归档到 tests/fixtures/real/<形态>/，
每份文件第一行加 `# device: <型号> <系统版本> <API Level>` 注释。

用法：
    python scripts/capture_real_fixtures.py [worker_url] [udid] [harmony_pc|harmony_mobile]
    默认: http://192.168.0.105:8088  3QC0124A10000066  harmony_pc
"""

import json
import re
import sys
import time
import urllib.request
from pathlib import Path

WORKER = sys.argv[1] if len(sys.argv) > 1 else "http://192.168.0.105:8088"
UDID = sys.argv[2] if len(sys.argv) > 2 else "3QC0124A10000066"
CATEGORY = sys.argv[3] if len(sys.argv) > 3 else "harmony_pc"

HDC = r'"D:\Test Worker\tools\hdc\hdc.exe"'
OUT_DIR = Path(__file__).resolve().parent.parent / "tests" / "fixtures" / "real" / CATEGORY


def run_remote(cmd: str, timeout: int = 90) -> str:
    """远程执行宿主机命令，返回 stdout（失败时抛异常）。"""
    payload = json.dumps(
        {
            "platform": "windows",
            "device_id": None,
            "actions": [{"action_type": "cmd_exec", "value": cmd}],
        }
    ).encode("utf-8")
    request = urllib.request.Request(
        f"{WORKER}/task/execute",
        data=payload,
        headers={"Content-Type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=timeout) as response:
        result = json.loads(response.read().decode("utf-8"))
    actions = result.get("actions") or [{}]
    action0 = actions[0]
    stdout = action0.get("stdout") or action0.get("output") or ""
    if result.get("status") != "success":
        raise RuntimeError(f"远程命令失败: {cmd}\n{result}")
    return stdout


def shell(cmd: str, timeout: int = 90) -> str:
    return run_remote(f'{HDC} -t {UDID} shell "{cmd}"', timeout)


def save(name: str, content: str, device_line: str) -> None:
    OUT_DIR.mkdir(parents=True, exist_ok=True)
    path = OUT_DIR / name
    # 规范：第一行设备指纹注释，命令输出保留原文（统一换行为 \n）
    normalized = content.replace("\r\n", "\n").replace("\r", "\n")
    path.write_text(f"{device_line}\n{normalized}", encoding="utf-8", newline="\n")
    print(f"  已保存 {path} ({len(normalized)} 字符)")


def main() -> None:
    print(f"Worker: {WORKER}  UDID: {UDID}  目录: {OUT_DIR}")

    model = shell("param get const.product.model").strip()
    version = shell("param get const.product.software.version").strip()
    api = shell("param get const.ohos.apiversion").strip()
    device_line = f"# device: {model} {version} API {api}"
    print(device_line)

    save("list_targets_v.txt", run_remote(f"{HDC} list targets -v"), device_line)
    save("hidumper_help.txt", shell("hidumper -h"), device_line)
    save("hidumper_cpuusage.txt", shell("hidumper --cpuusage", timeout=120), device_line)
    save("hidumper_mem.txt", shell("hidumper --mem", timeout=120), device_line)

    ps_output = shell("ps -ef")
    save("ps_ef.txt", ps_output, device_line)

    save("proc_stat_t0.txt", shell("cat /proc/stat"), device_line)
    time.sleep(2)
    save("proc_stat_t1.txt", shell("cat /proc/stat"), device_line)
    save("proc_meminfo.txt", shell("cat /proc/meminfo"), device_line)
    save("proc_net_dev.txt", shell("cat /proc/net/dev"), device_line)
    save("which_tools.txt", shell("which hiperf; which top"), device_line)

    # 从 ps -ef 里挑一个有效 PID 抓进程级内存
    pid = None
    for line in ps_output.splitlines()[1:]:
        columns = re.split(r"\s+", line.strip())
        if len(columns) > 2 and columns[1].isdigit() and int(columns[1]) > 1:
            pid = columns[1]
            break
    if pid:
        print(f"  选取 PID={pid} 抓进程级内存")
        save("hidumper_mem_pid.txt", shell(f"hidumper --mem {pid}", timeout=120), device_line)
    else:
        print("  未能从 ps -ef 选出 PID，跳过 hidumper_mem_pid")

    print("冻结完成。")


if __name__ == "__main__":
    main()
