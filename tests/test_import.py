import perfharmony


def test_import_and_version():
    assert perfharmony.__version__ == "0.1.1"
    assert callable(perfharmony.list_targets)
    assert callable(perfharmony.list_sensors)
