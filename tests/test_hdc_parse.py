from pathlib import Path

import perfharmony


def test_list_targets_fixture_is_parsed():
    fixture = Path(__file__).parent / "fixtures" / "list_targets_v.txt"
    targets = perfharmony.parse_targets_for_test(fixture.read_text(encoding="utf-8"))

    assert [target["udid"] for target in targets] == [
        "127.0.0.1:5555",
        "ABCDEF123456",
        "COM1",
        "ZXCVBN098765",
        "3QC0124A10000066",
    ]
    assert targets[0]["connection_type"] == "TCP"
    # 真机（华为 MateBook 2in1）输出：tab 分隔 + Connected 状态
    assert targets[-1]["status"] == "Connected"
    assert targets[3]["status"] == "Offline"
