use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::PyTypeInfo;

mod data;
mod error;
mod hdc;
mod monitor;
mod parse;
mod ring_buffer;

use hdc::{parse_list_targets, HdcClient};
use monitor::{MonitorInner, MonitorResultInner};
use parse::{build_system_sensors, parse_blocks, parse_ps_ef, sp_daemon_command};

#[pyclass]
#[derive(Clone, Debug)]
pub struct TargetInfo {
    #[pyo3(get)]
    pub udid: String,
    #[pyo3(get)]
    pub status: String,
    #[pyo3(get)]
    pub connection_type: String,
}

impl From<hdc::TargetInfo> for TargetInfo {
    fn from(value: hdc::TargetInfo) -> Self {
        Self {
            udid: value.udid,
            status: value.status,
            connection_type: value.connection_type,
        }
    }
}

#[pyclass(name = "HdcErrorKind")]
pub struct PyHdcErrorKind;

#[pymethods]
impl PyHdcErrorKind {
    #[classattr]
    const NOT_FOUND: &'static str = "not_found";
    #[classattr]
    const TARGET_OFFLINE: &'static str = "target_offline";
    #[classattr]
    const TIMEOUT: &'static str = "timeout";
    #[classattr]
    const COMMAND_FAILED: &'static str = "command_failed";
}

#[pyclass(name = "MonitorResult")]
pub struct PyMonitorResult {
    pub(crate) result: MonitorResultInner,
}

#[pymethods]
impl PyMonitorResult {
    #[getter]
    fn samples(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let values = pyo3::types::PyList::empty_bound(py);
        for sample in &self.result.samples {
            values.append(Py::new(
                py,
                PySample {
                    sample: sample.clone(),
                },
            )?)?;
        }
        Ok(values.into())
    }

    fn to_dicts(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        let values = pyo3::types::PyList::empty_bound(py);
        for sample in &self.result.samples {
            values.append(PySample::sample_to_dict(py, sample)?)?;
        }
        Ok(values.into())
    }
}

#[pyclass(name = "Sample")]
#[derive(Clone)]
pub struct PySample {
    pub(crate) sample: data::Sample,
}

#[pymethods]
impl PySample {
    #[getter]
    fn sequence(&self) -> u64 {
        self.sample.sequence
    }

    #[getter]
    fn elapsed_ms(&self) -> u64 {
        self.sample.elapsed_ms
    }

    #[getter]
    fn timestamp(&self) -> String {
        self.sample.timestamp.to_rfc3339()
    }

    #[getter]
    fn system(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        system_to_dict(py, &self.sample.system)
    }

    #[getter]
    fn hwinfo_raw(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        hwinfo_to_dict(py, &self.sample.hwinfo_raw)
    }

    #[getter]
    fn processes(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        process_list_to_py(py, self.sample.processes.as_ref())
    }

    #[getter]
    fn aggregated(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        aggregated_to_py(py, self.sample.aggregated.as_ref())
    }

    #[getter]
    fn top_n_cpu(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        aggregated_to_py(py, self.sample.top_n_cpu.as_ref())
    }

    #[getter]
    fn top_n_gpu(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        aggregated_to_py(py, self.sample.top_n_gpu.as_ref())
    }

    fn to_dict(&self, py: Python<'_>) -> PyResult<Py<PyAny>> {
        Self::sample_to_dict(py, &self.sample)
    }
}

impl PySample {
    fn sample_to_dict(py: Python<'_>, sample: &data::Sample) -> PyResult<Py<PyAny>> {
        let value = pyo3::types::PyDict::new_bound(py);
        value.set_item("sequence", sample.sequence)?;
        value.set_item("elapsed_ms", sample.elapsed_ms)?;
        value.set_item("timestamp", sample.timestamp.to_rfc3339())?;
        value.set_item("system", system_to_dict(py, &sample.system)?)?;
        value.set_item("hwinfo_raw", hwinfo_to_dict(py, &sample.hwinfo_raw)?)?;
        value.set_item(
            "processes",
            process_list_to_py(py, sample.processes.as_ref())?,
        )?;
        value.set_item(
            "aggregated",
            aggregated_to_py(py, sample.aggregated.as_ref())?,
        )?;
        value.set_item(
            "top_n_cpu",
            aggregated_to_py(py, sample.top_n_cpu.as_ref())?,
        )?;
        value.set_item(
            "top_n_gpu",
            aggregated_to_py(py, sample.top_n_gpu.as_ref())?,
        )?;
        Ok(value.into())
    }
}

fn system_to_dict(py: Python<'_>, system: &data::SystemMetrics) -> PyResult<Py<PyAny>> {
    let value = pyo3::types::PyDict::new_bound(py);
    value.set_item("cpu_percent", system.cpu_percent)?;
    value.set_item("gpu_percent", system.gpu_percent)?;
    value.set_item("gpu_source", &system.gpu_source)?;
    let adapters = pyo3::types::PyList::empty_bound(py);
    for adapter in &system.gpu_adapters {
        let item = pyo3::types::PyDict::new_bound(py);
        item.set_item("luid", &adapter.luid)?;
        item.set_item("name", &adapter.name)?;
        item.set_item("utilization_percent", adapter.utilization_percent)?;
        adapters.append(item)?;
    }
    value.set_item("gpu_adapters", adapters)?;
    Ok(value.into())
}

fn hwinfo_to_dict(
    py: Python<'_>,
    raw: &std::collections::HashMap<String, data::SensorValue>,
) -> PyResult<Py<PyAny>> {
    let value = pyo3::types::PyDict::new_bound(py);
    for (key, sensor) in raw {
        let item = pyo3::types::PyDict::new_bound(py);
        item.set_item("value", sensor.value)?;
        item.set_item("unit", &sensor.unit)?;
        value.set_item(key, item)?;
    }
    Ok(value.into())
}

fn process_list_to_py(
    py: Python<'_>,
    processes: Option<&Vec<data::ProcessInfo>>,
) -> PyResult<Py<PyAny>> {
    let Some(processes) = processes else {
        return Ok(py.None());
    };
    let values = pyo3::types::PyList::empty_bound(py);
    for process in processes {
        let item = pyo3::types::PyDict::new_bound(py);
        item.set_item("pid", process.pid)?;
        item.set_item("name", &process.name)?;
        item.set_item("cpu_percent", process.cpu_percent)?;
        item.set_item("working_set_mb", process.working_set_mb)?;
        item.set_item("committed_memory_mb", process.committed_memory_mb)?;
        item.set_item("gpu_percent", process.gpu_percent)?;
        item.set_item("gpu_memory_mb", process.gpu_memory_mb)?;
        item.set_item("handle_count", process.handle_count)?;
        values.append(item)?;
    }
    Ok(values.into())
}

fn aggregated_to_py(
    py: Python<'_>,
    values: Option<&Vec<data::AggregatedProcessInfo>>,
) -> PyResult<Py<PyAny>> {
    let Some(values) = values else {
        return Ok(py.None());
    };
    let result = pyo3::types::PyList::empty_bound(py);
    for item_value in values {
        let item = pyo3::types::PyDict::new_bound(py);
        item.set_item("name", &item_value.name)?;
        item.set_item("pids", &item_value.pids)?;
        item.set_item("cpu_percent_total", item_value.cpu_percent_total)?;
        item.set_item("working_set_mb_total", item_value.working_set_mb_total)?;
        item.set_item(
            "committed_memory_mb_total",
            item_value.committed_memory_mb_total,
        )?;
        item.set_item("gpu_percent_total", item_value.gpu_percent_total)?;
        item.set_item("handle_count_total", item_value.handle_count_total)?;
        item.set_item("process_count", item_value.process_count)?;
        result.append(item)?;
    }
    Ok(result.into())
}

#[pyfunction]
#[pyo3(signature = (hdc_path=None, include_uart=false))]
fn list_targets(
    hdc_path: Option<String>,
    include_uart: bool,
) -> PyResult<Vec<HashMap<String, String>>> {
    let client = HdcClient::from_optional_path(hdc_path).map_err(error::to_py_err)?;
    let output = client.list_targets_raw().map_err(error::to_py_err)?;
    Ok(parse_list_targets(&output)
        .into_iter()
        .filter(|target| include_uart || target.connection_type.to_ascii_uppercase() != "UART")
        .map(|target| {
            HashMap::from([
                ("udid".to_string(), target.udid),
                ("status".to_string(), target.status),
                ("connection_type".to_string(), target.connection_type),
            ])
        })
        .collect())
}

#[pyfunction]
fn parse_targets_for_test(output: &str) -> Vec<HashMap<String, String>> {
    parse_list_targets(output)
        .into_iter()
        .map(|target| {
            HashMap::from([
                ("udid".to_string(), target.udid),
                ("status".to_string(), target.status),
                ("connection_type".to_string(), target.connection_type),
            ])
        })
        .collect()
}

/// 设备级清理：收掉设备端所有 SP_daemon 采集实例（官方 -stop）。
///
/// 供多实例并发采集（多包名）的上层在启动前/全部结束后统一调用；
/// 单实例 Monitor 默认自动清理，无需调用本函数。
#[pyfunction]
#[pyo3(signature = (udid, hdc_path=None))]
fn stop_daemon(udid: String, hdc_path: Option<String>) -> PyResult<()> {
    if udid.trim().is_empty() {
        return Err(pyo3::exceptions::PyValueError::new_err("udid 不能为空"));
    }
    let client = HdcClient::from_optional_path(hdc_path).map_err(error::to_py_err)?;
    let cancel = std::sync::atomic::AtomicBool::new(false);
    client
        .shell_with_cancel(&udid, "SP_daemon -stop", 10, &cancel)
        .map_err(error::to_py_err)?;
    Ok(())
}

#[pyfunction]
#[pyo3(signature = (udid, hdc_path=None))]
fn list_processes(udid: String, hdc_path: Option<String>) -> PyResult<Vec<(u32, String)>> {
    let client = HdcClient::from_optional_path(hdc_path).map_err(error::to_py_err)?;
    let output = client
        .shell(&udid, "ps -ef", 15)
        .map_err(error::to_py_err)?;
    Ok(parse::parse_ps_ef(&output))
}

/// 探测当前设备能够解析出的稳定传感器键。
///
/// 0.2.0 起基于 SP_daemon：跑 2 拍（首拍是差分基线），取末拍白名单传感器键。
#[pyfunction]
#[pyo3(signature = (udid, hdc_path=None))]
fn list_sensors(udid: String, hdc_path: Option<String>) -> PyResult<Vec<String>> {
    if udid.trim().is_empty() {
        return Err(pyo3::exceptions::PyValueError::new_err("udid 不能为空"));
    }
    let client = HdcClient::from_optional_path(hdc_path).map_err(error::to_py_err)?;
    let command = sp_daemon_command(2, None, None);
    let text = client
        .shell(&udid, &command, 30)
        .map_err(error::to_py_err)?;
    let blocks = parse_blocks(&text);
    let Some(block) = blocks.last() else {
        return Ok(Vec::new());
    };
    let mut keys = build_system_sensors(block).into_keys().collect::<Vec<_>>();
    keys.sort();
    Ok(keys)
}

#[pyfunction]
#[pyo3(signature = (udid, hdc_path=None))]
fn probe(udid: String, hdc_path: Option<String>, py: Python<'_>) -> PyResult<Py<PyAny>> {
    if udid.trim().is_empty() {
        return Err(pyo3::exceptions::PyValueError::new_err("udid 不能为空"));
    }
    let client = HdcClient::from_optional_path(hdc_path).map_err(error::to_py_err)?;
    let targets = client.list_targets_raw().map_err(error::to_py_err)?;
    let target = parse_list_targets(&targets)
        .into_iter()
        .find(|item| item.udid == udid);
    let result = pyo3::types::PyDict::new_bound(py);
    let capabilities = pyo3::types::PyDict::new_bound(py);

    let Some(target) = target else {
        result.set_item("udid", &udid)?;
        result.set_item(
            "target",
            capability_to_py(py, "unavailable", Some("目标不存在"))?,
        )?;
        result.set_item("capabilities", capabilities)?;
        return Ok(result.into());
    };
    if !target.is_usable() {
        result.set_item("udid", &udid)?;
        result.set_item(
            "target",
            capability_to_py(
                py,
                "unavailable",
                Some(&format!("目标状态: {}", target.status)),
            )?,
        )?;
        result.set_item("capabilities", capabilities)?;
        return Ok(result.into());
    }

    result.set_item("udid", &udid)?;
    result.set_item(
        "target",
        capability_to_py(py, "available", Some("HDC target ready"))?,
    )?;

    // SP_daemon 探测：能解析出块且末拍有白名单传感器即可用。
    // 注意设备端命令绝不能接管道（SP_daemon 检测到非终端会静默不输出）。
    let sp_status = match client.shell(&udid, &sp_daemon_command(2, None, None), 30) {
        Ok(text) => {
            let blocks = parse_blocks(&text);
            match blocks.last() {
                Some(block) if !build_system_sensors(block).is_empty() => {
                    ("available".to_string(), None)
                }
                Some(_) => (
                    "degraded".to_string(),
                    Some("SP_daemon 有输出但未解析出白名单指标".to_string()),
                ),
                None => (
                    "unavailable".to_string(),
                    Some("SP_daemon 无数据输出（设备可能不支持）".to_string()),
                ),
            }
        }
        Err(error) => ("unavailable".to_string(), Some(error.to_string())),
    };
    capabilities.set_item(
        "sp_daemon",
        capability_to_py(py, &sp_status.0, sp_status.1.as_deref())?,
    )?;

    // 进程列表（进程选择器用，不在采集循环内）。
    let process_status = match client.shell(&udid, "ps -ef", 15) {
        Ok(text) => {
            if parse_ps_ef(&text).is_empty() {
                (
                    "degraded".to_string(),
                    Some("ps -ef 未解析到进程".to_string()),
                )
            } else {
                ("available".to_string(), None)
            }
        }
        Err(error) => ("unavailable".to_string(), Some(error.to_string())),
    };
    capabilities.set_item(
        "process",
        capability_to_py(py, &process_status.0, process_status.1.as_deref())?,
    )?;

    result.set_item("capabilities", capabilities)?;
    Ok(result.into())
}

fn capability_to_py(py: Python<'_>, status: &str, reason: Option<&str>) -> PyResult<Py<PyAny>> {
    let value = pyo3::types::PyDict::new_bound(py);
    value.set_item("status", status)?;
    value.set_item("available", status == "available")?;
    value.set_item("degraded", status == "degraded")?;
    value.set_item("unavailable", status == "unavailable")?;
    value.set_item("reason", reason)?;
    Ok(value.into())
}

#[pyclass(name = "Monitor")]
pub struct PyMonitor {
    inner: std::sync::Arc<MonitorInner>,
    config: Option<monitor::MonitorConfigOwned>,
    handle: std::sync::Mutex<Option<std::thread::JoinHandle<()>>>,
}

#[pymethods]
impl PyMonitor {
    #[new]
    // 0.3.0：-PKG 单应用（含子进程）与 -PID 单进程精准模式二选一。
    #[pyo3(signature = (udid, interval=1.0, duration=None, hdc_path=None, package=None, pid=None, auto_cleanup=None))]
    #[allow(clippy::too_many_arguments)]
    fn new(
        udid: String,
        interval: f64,
        duration: Option<f64>,
        hdc_path: Option<String>,
        package: Option<String>,
        pid: Option<u32>,
        auto_cleanup: Option<bool>,
    ) -> PyResult<Self> {
        if udid.trim().is_empty() {
            return Err(pyo3::exceptions::PyValueError::new_err("udid 不能为空"));
        }
        if interval < 1.0 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "interval 必须大于等于 1 秒",
            ));
        }
        if duration.is_some_and(|value| value <= 0.0) {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "duration 必须大于 0",
            ));
        }
        if package
            .as_ref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "package 不能为空字符串",
            ));
        }
        if package.is_some() && pid.is_some() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "package 与 pid 只能二选一",
            ));
        }
        if pid.is_some_and(|value| value == 0) {
            return Err(pyo3::exceptions::PyValueError::new_err("pid 必须为正整数"));
        }
        Ok(Self {
            inner: std::sync::Arc::new(MonitorInner::new()),
            config: Some(monitor::MonitorConfigOwned {
                udid,
                hdc_path,
                interval,
                duration,
                package,
                pid,
                // 多实例并发采集时由上层传 false 并统一协调 -stop 清理。
                auto_cleanup: auto_cleanup.unwrap_or(true),
            }),
            handle: std::sync::Mutex::new(None),
        })
    }

    fn start(&self) -> PyResult<()> {
        let config = self.config.clone().expect("Monitor 配置必须存在");
        let client =
            HdcClient::from_optional_path(config.hdc_path.clone()).map_err(error::to_py_err)?;
        let targets = client.list_targets_raw().map_err(error::to_py_err)?;
        let target = parse_list_targets(&targets)
            .into_iter()
            .find(|item| item.udid == config.udid);
        if target.as_ref().is_none_or(|item| !item.is_usable()) {
            return Err(error::to_py_err(error::Error::TargetOffline(config.udid)));
        }

        self.stop();
        self.inner.reset();
        let inner = self.inner.clone();
        let handle = std::thread::Builder::new()
            .name("perfharmony-monitor".to_string())
            .spawn(move || monitor::run_monitor(config, client, inner))
            .map_err(|error| pyo3::exceptions::PyRuntimeError::new_err(error.to_string()))?;
        *self.handle.lock().expect("Monitor handle mutex poisoned") = Some(handle);
        Ok(())
    }

    fn stop(&self) {
        self.inner.request_stop();
        if let Some(handle) = self
            .handle
            .lock()
            .expect("Monitor handle mutex poisoned")
            .take()
        {
            let _ = handle.join();
        }
    }

    fn is_running(&self) -> bool {
        self.inner.is_running()
    }

    fn buffer_len(&self) -> usize {
        self.inner.buffer_len()
    }

    fn get_result(&self) -> PyMonitorResult {
        PyMonitorResult {
            result: self.inner.drain(),
        }
    }

    #[getter]
    fn last_error(&self) -> Option<String> {
        self.inner.last_error()
    }

    fn __enter__(slf: PyRef<'_, Self>) -> PyResult<PyRef<'_, Self>> {
        slf.start()?;
        Ok(slf)
    }

    #[pyo3(signature = (_ty=None, _value=None, _traceback=None))]
    fn __exit__(
        &self,
        _ty: Option<Py<PyAny>>,
        _value: Option<Py<PyAny>>,
        _traceback: Option<Py<PyAny>>,
    ) {
        self.stop();
    }
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("HdcError", error::HdcError::type_object_bound(m.py()))?;
    m.add_class::<PyHdcErrorKind>()?;
    m.add_class::<PyMonitorResult>()?;
    m.add_class::<PySample>()?;
    m.add_class::<PyMonitor>()?;
    m.add_class::<TargetInfo>()?;
    m.add_function(wrap_pyfunction!(list_targets, m)?)?;
    m.add_function(wrap_pyfunction!(parse_targets_for_test, m)?)?;
    m.add_function(wrap_pyfunction!(list_processes, m)?)?;
    m.add_function(wrap_pyfunction!(stop_daemon, m)?)?;
    m.add_function(wrap_pyfunction!(list_sensors, m)?)?;
    m.add_function(wrap_pyfunction!(probe, m)?)?;
    Ok(())
}
