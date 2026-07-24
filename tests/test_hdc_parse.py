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
    ]
    assert targets[0]["connection_type"] == "TCP"
    assert targets[-1]["status"] == "Offline"
