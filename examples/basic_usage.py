"""perfharmony 最小使用示例。

运行前请准备 HDC、一个 Ready 状态的鸿蒙设备，并将 UDID 替换为实际值。
"""

import perfharmony


def main() -> None:
    """列出能力并采集一个短时间窗口。"""
    udid = "替换为 HDC UDID"
    print("可用传感器:", perfharmony.list_sensors(udid))
    print("设备能力:", perfharmony.probe(udid))

    # 模糊模式：-PKG 按包名采集（自动含主进程 + 子进程）。
    with perfharmony.Monitor(
        udid=udid,
        interval=2.0,
        duration=10.0,
        package="com.example.app",
    ) as monitor:
        while monitor.is_running():
            result = monitor.get_result()
            for sample in result.samples:
                print(sample.sequence, sample.elapsed_ms, sample.system)


def exact_mode_example() -> None:
    """精准模式示例：先自行解析 PID（如 ps -ef），再用 -PID 单进程采集。"""
    udid = "替换为 HDC UDID"
    pid = 12345  # 通过 hdc shell ps -ef 定位到的目标进程 PID
    with perfharmony.Monitor(
        udid=udid,
        interval=2.0,
        duration=10.0,
        pid=pid,  # 与 package 互斥
    ) as monitor:
        while monitor.is_running():
            result = monitor.get_result()
            for sample in result.samples:
                print(sample.sequence, sample.elapsed_ms, sample.system)


if __name__ == "__main__":
    main()
