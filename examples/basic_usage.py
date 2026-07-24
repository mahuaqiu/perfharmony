"""perfharmony 最小使用示例。

运行前请准备 HDC、一个 Ready 状态的鸿蒙设备，并将 UDID 替换为实际值。
"""

import perfharmony


def main() -> None:
    """列出能力并采集一个短时间窗口。"""
    udid = "替换为 HDC UDID"
    print("可用传感器:", perfharmony.list_sensors(udid))
    print("设备能力:", perfharmony.probe(udid))

    process_filter = perfharmony.ProcessFilter(names=["com.example.app"])
    with perfharmony.Monitor(
        udid=udid,
        interval=2.0,
        duration=10.0,
        process_filter=process_filter,
    ) as monitor:
        while monitor.is_running():
            result = monitor.get_result()
            for sample in result.samples:
                print(sample.sequence, sample.elapsed_ms, sample.system)


if __name__ == "__main__":
    main()
