"""
perfwin 兼容契约测试骨架。

本文件定义 perfharmony 与 perfwin 共享的可观察行为契约。
已实现的公共 API 契约直接通过；依赖真实 HDC target 的 Monitor 启停和样本字段测试保留为待真机/FakeHdc 验证。

契约覆盖：
  - Monitor 构造参数校验（0.2.0：interval>=1、package 单应用）
  - Monitor 上下文管理器协议
  - get_result().samples 结构
  - to_dicts() 稳定公共 API
  - Sample dict 属性完整性
  - ISO 8601 时间戳
  - 增量 drain（get_result 调用后缓冲清空）
  - buffer_len / is_running 行为
"""

import pytest

# Monitor 由 PyO3 扩展提供；导入失败时仍允许解析测试文件。
try:
    import perfharmony
    _HAS_MONITOR = hasattr(perfharmony, "Monitor")
except ImportError:
    _HAS_MONITOR = False


# ---------------------------------------------------------------------------
# Monitor 构造参数契约（0.2.0：SP_daemon 唯一数据源）
# ---------------------------------------------------------------------------

def test_monitor_rejects_empty_udid():
    """udid 为空必须报错。"""
    with pytest.raises(ValueError):
        perfharmony.Monitor(udid="")


def test_monitor_rejects_sub_second_interval():
    """SP_daemon 固定 1s 一拍，interval 必须 >= 1 秒。"""
    with pytest.raises(ValueError):
        perfharmony.Monitor(udid="TEST_UDID_FAKE", interval=0.5)


def test_monitor_rejects_non_positive_duration():
    """duration 必须大于 0。"""
    with pytest.raises(ValueError):
        perfharmony.Monitor(udid="TEST_UDID_FAKE", interval=2.0, duration=0)


def test_monitor_rejects_blank_package():
    """package 不能是空字符串（一次采集限一个应用）。"""
    with pytest.raises(ValueError):
        perfharmony.Monitor(udid="TEST_UDID_FAKE", interval=2.0, package="  ")


def test_monitor_accepts_package():
    """package 传入合法包名可构造。"""
    monitor = perfharmony.Monitor(
        udid="TEST_UDID_FAKE", interval=2.0, package="com.example.app"
    )
    assert monitor is not None


def test_process_filter_removed_in_020():
    """0.2.0 已删除 ProcessFilter（改用 package 单应用采集）。"""
    assert not hasattr(perfharmony, "ProcessFilter")


def _pid_param_supported() -> bool:
    """0.3.0 wheel 才有 pid 参数；旧 wheel 安装态下跳过相关契约。"""
    if not _HAS_MONITOR:
        return False
    try:
        perfharmony.Monitor(udid="TEST_UDID_FAKE", interval=2.0, pid=1)
    except TypeError:
        return False
    except ValueError:
        return True
    return True


_PID_PARAM = _pid_param_supported()


@pytest.mark.skipif(not _PID_PARAM, reason="等待 0.3.0 wheel 提供 pid 参数")
def test_monitor_accepts_pid():
    """pid 传入正整数可构造（-PID 精准模式）。"""
    monitor = perfharmony.Monitor(udid="TEST_UDID_FAKE", interval=2.0, pid=123)
    assert monitor is not None


@pytest.mark.skipif(not _PID_PARAM, reason="等待 0.3.0 wheel 提供 pid 参数")
def test_monitor_rejects_zero_pid():
    """pid 必须为正整数。"""
    with pytest.raises(ValueError):
        perfharmony.Monitor(udid="TEST_UDID_FAKE", interval=2.0, pid=0)


@pytest.mark.skipif(not _PID_PARAM, reason="等待 0.3.0 wheel 提供 pid 参数")
def test_pid_and_package_mutually_exclusive():
    """package 与 pid 只能二选一（一次采集一个 -PKG 或一个 -PID）。"""
    with pytest.raises(ValueError):
        perfharmony.Monitor(
            udid="TEST_UDID_FAKE", interval=2.0, package="com.example.app", pid=123
        )


# ---------------------------------------------------------------------------
# Monitor 上下文管理器协议
# ---------------------------------------------------------------------------

@pytest.mark.xfail(reason="需要真实或可注入的 HDC target 才能验证 Monitor 采集", strict=True)
def test_monitor_context_manager(fake_udid="TEST_UDID_FAKE"):
    """Monitor 支持 with 语句，退出后 is_running() == False。"""
    with perfharmony.Monitor(udid=fake_udid, interval=2.0) as monitor:
        assert monitor.is_running()
    assert not monitor.is_running()


@pytest.mark.xfail(reason="需要真实或可注入的 HDC target 才能验证 Monitor 采集", strict=True)
def test_monitor_start_stop(fake_udid="TEST_UDID_FAKE"):
    """start() / stop() 手动调用与 is_running() 状态一致。"""
    monitor = perfharmony.Monitor(udid=fake_udid, interval=2.0)
    assert not monitor.is_running()
    monitor.start()
    assert monitor.is_running()
    monitor.stop()
    assert not monitor.is_running()


def test_monitor_buffer_len_initial(fake_udid="TEST_UDID_FAKE"):
    """构造后 buffer_len() == 0。"""
    monitor = perfharmony.Monitor(udid=fake_udid, interval=2.0)
    assert monitor.buffer_len() == 0


# ---------------------------------------------------------------------------
# get_result / to_dicts / 增量 drain
# ---------------------------------------------------------------------------

def test_get_result_returns_samples(fake_udid="TEST_UDID_FAKE"):
    """get_result() 返回带 .samples 属性的对象。"""
    monitor = perfharmony.Monitor(udid=fake_udid, interval=2.0)
    result = monitor.get_result()
    assert hasattr(result, "samples")
    assert isinstance(result.samples, list)


def test_get_result_drains_buffer(fake_udid="TEST_UDID_FAKE"):
    """连续两次 get_result() 第二次应返回空列表（增量 drain）。"""
    monitor = perfharmony.Monitor(udid=fake_udid, interval=2.0)
    monitor.get_result()
    second = monitor.get_result()
    assert second.samples == []


def test_to_dicts_is_stable_api(fake_udid="TEST_UDID_FAKE"):
    """MonitorResult 具备 to_dicts() 方法，返回 list[dict]。"""
    monitor = perfharmony.Monitor(udid=fake_udid, interval=2.0)
    result = monitor.get_result()
    dicts = result.to_dicts()
    assert isinstance(dicts, list)


# ---------------------------------------------------------------------------
# Sample 字段完整性（Worker 侧必读属性）
# ---------------------------------------------------------------------------

@pytest.mark.xfail(reason="需要真实或可注入的 HDC target 才能验证 Monitor 采集", strict=True)
def test_sample_dict_required_keys():
    """
    Sample dict 必须包含 Worker 所读取的所有键。
    此测试使用 FakeHdc 注入或 mock 产出单个 Sample 后验证。
    """
    # TODO Task 4: 注入 FakeHdc，产出一个 Sample，验证键集合
    required_keys = {
        "sequence", "elapsed_ms", "timestamp",
        "system", "hwinfo_raw",
        "processes", "aggregated", "top_n_cpu", "top_n_gpu",
    }
    # sample_dict = ... (Task 4 实现)
    # assert required_keys.issubset(sample_dict.keys())
    pytest.skip("需要将 FakeHdc 注入 PyO3 Monitor 后再验证样本字段")


@pytest.mark.xfail(reason="需要真实或可注入的 HDC target 才能验证 Monitor 采集", strict=True)
def test_sample_sequence_monotonic():
    """sequence 从 1 开始单调递增。"""
    # TODO Task 4: 验证两个样本的 sequence 差为 1
    pytest.skip("需要将 FakeHdc 注入 PyO3 Monitor 后再验证样本序列")


@pytest.mark.xfail(reason="需要真实或可注入的 HDC target 才能验证 Monitor 采集", strict=True)
def test_sample_timestamp_iso8601():
    """timestamp 为 ISO 8601 格式字符串。"""
    import re
    iso_re = re.compile(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}")
    # TODO Task 4: 从 sample dict 取 timestamp 并验证格式
    pytest.skip("需要将 FakeHdc 注入 PyO3 Monitor 后再验证时间戳")


@pytest.mark.xfail(reason="需要真实或可注入的 HDC target 才能验证 Monitor 采集", strict=True)
def test_sample_elapsed_ms_nonnegative():
    """elapsed_ms 非负。"""
    pytest.skip("需要将 FakeHdc 注入 PyO3 Monitor 后再验证 elapsed_ms")


@pytest.mark.xfail(reason="需要真实或可注入的 HDC target 才能验证 Monitor 采集", strict=True)
def test_sample_cpu_in_range():
    """system.cpu_percent 若不为 None，必须在 [0, 100]。"""
    pytest.skip("需要将 FakeHdc 注入 PyO3 Monitor 后再验证 CPU 归一化")
