use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::types::PyDict;
use pyo3::PyTypeInfo;

mod collector;
mod data;
mod error;
mod hdc;
mod monitor;
mod parse;
mod ring_buffer;

use hdc::{parse_list_targets, HdcClient};
use monitor::{MonitorInner, MonitorResultInner};
use parse::{parse_hidumper_cpuusage, parse_hidumper_mem_system, parse_proc_meminfo, parse_ps_ef};

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
    #[classattr]
    const PARSE_FAILED: &'static str = "parse_failed";
}

#[pyclass(name = "ProcessFilter")]
#[derive(Clone, Debug)]
pub struct PyProcessFilter {
    pub(crate) filter: data::ProcessFilter,
}

#[pymethods]
impl PyProcessFilter {
    #[new]
    #[pyo3(signature = (pids=None, name=None, names=None, name_regex=None))]
    fn new(
        pids: Option<Vec<u32>>,
        name: Option<String>,
        names: Option<Vec<String>>,
        name_regex: Option<String>,
    ) -> PyResult<Self> {
        let supplied = [
            pids.is_some(),
            name.is_some(),
            names.is_some(),
            name_regex.is_some(),
        ]
        .into_iter()
        .filter(|value| *value)
        .count();
        if supplied != 1 {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "ProcessFilter 必须且只能指定 pids、name、names 或 name_regex 之一",
            ));
        }
        let filter = if let Some(values) = pids {
            if values.is_empty() {
                return Err(pyo3::exceptions::PyValueError::new_err("pids 不能为空"));
            }
            data::ProcessFilter::Pids(values)
        } else if let Some(value) = name {
            if value.trim().is_empty() {
                return Err(pyo3::exceptions::PyValueError::new_err("name 不能为空"));
            }
            data::ProcessFilter::Name(value)
        } else if let Some(values) = names {
            if values.is_empty() {
                return Err(pyo3::exceptions::PyValueError::new_err("names 不能为空"));
            }
            data::ProcessFilter::Names(values)
        } else {
            let value = name_regex.expect("已校验 name_regex 存在");
            regex::Regex::new(&value).map_err(|error| {
                pyo3::exceptions::PyValueError::new_err(format!("name_regex 无效: {error}"))
            })?;
            data::ProcessFilter::NameRegex(value)
        };
        Ok(Self { filter })
    }
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
/// 这是调试辅助 API，不把任意命令输出直接暴露给调用方；命令或字段不可用时只跳过
/// 对应能力，HDC 路径解析失败仍返回可识别异常。
#[pyfunction]
#[pyo3(signature = (udid, hdc_path=None))]
fn list_sensors(udid: String, hdc_path: Option<String>) -> PyResult<Vec<String>> {
    if udid.trim().is_empty() {
        return Err(pyo3::exceptions::PyValueError::new_err("udid 不能为空"));
    }
    let client = HdcClient::from_optional_path(hdc_path).map_err(error::to_py_err)?;
    let mut raw = HashMap::new();
    let mut optional_state = collector::OptionalState::default();

    if let Ok(text) = client.shell(&udid, "hidumper --cpuusage", 15) {
        if let Ok(cpu) = parse_hidumper_cpuusage(&text) {
            collector::add_sensor(&mut raw, "Harmony CPU Usage", cpu.total_percent, "%");
            if let Some(value) = cpu.user_percent {
                collector::add_sensor(&mut raw, "Harmony CPU User", value, "%");
            }
            if let Some(value) = cpu.system_percent {
                collector::add_sensor(&mut raw, "Harmony CPU System", value, "%");
            }
            if let Some(value) = cpu.idle_percent {
                collector::add_sensor(&mut raw, "Harmony CPU Idle", value, "%");
            }
        }
    }

    if let Ok(text) = client.shell(&udid, "hidumper --mem", 15) {
        if let Ok(mem) = parse_hidumper_mem_system(&text) {
            collector::add_sensor(
                &mut raw,
                "Harmony Mem Total",
                mem.total_kb as f64 / 1024.0,
                "MB",
            );
            collector::add_sensor(
                &mut raw,
                "Harmony Mem Used",
                mem.used_kb as f64 / 1024.0,
                "MB",
            );
            collector::add_sensor(
                &mut raw,
                "Harmony Mem Free",
                mem.free_kb as f64 / 1024.0,
                "MB",
            );
            if let Some(value) = mem.available_kb {
                collector::add_sensor(
                    &mut raw,
                    "Harmony Mem Available",
                    value as f64 / 1024.0,
                    "MB",
                );
            }
        }
    }

    if let Some(command) = collector::optional_command(true, true, true, true, true) {
        if let Ok(text) = client.shell(&udid, &command, 15) {
            let sections = collector::split_optional_output(&text);
            if let Some(section) = sections.get("network") {
                let _ = collector::collect_network(section, &mut optional_state, 1.0, &mut raw);
            }
            if let Some(section) = sections.get("power") {
                let _ = collector::collect_power(section, &mut raw);
            }
            if let Some(section) = sections.get("thermal") {
                let _ = collector::collect_thermal(section, &mut raw);
            }
            if let Some(section) = sections.get("gpu") {
                let mut sample = data::Sample {
                    sequence: 0,
                    elapsed_ms: 0,
                    system: Default::default(),
                    timestamp: chrono::Utc::now(),
                    hwinfo_raw: HashMap::new(),
                    processes: None,
                    aggregated: None,
                    top_n_cpu: None,
                    top_n_gpu: None,
                };
                if collector::collect_gpu(section, &mut sample).is_ok() {
                    raw.extend(sample.hwinfo_raw);
                }
            }
            if let Some(section) = sections.get("cpufreq") {
                let _ = collector::collect_cpufreq(section, &mut raw);
            }
        }
    }

    let mut keys = raw.into_keys().collect::<Vec<_>>();
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

    let cpu_output = client.shell(&udid, "hidumper --cpuusage", 15);
    let cpu_status = match cpu_output {
        Ok(text) => collector::classify_probe(
            Ok(text.clone()),
            parse_hidumper_cpuusage(&text).map(|_| ()),
            "hidumper --cpuusage",
        ),
        Err(error) => match client.shell(&udid, "cat /proc/stat", 15) {
            Ok(text) => collector::CapabilityStatus {
                status: "degraded".to_string(),
                reason: Some(format!(
                    "hidumper --cpuusage 不可用，/proc/stat 可用: {error}; {}",
                    text.lines().next().unwrap_or("")
                )),
            },
            Err(fallback_error) => collector::CapabilityStatus {
                status: "unavailable".to_string(),
                reason: Some(format!(
                    "hidumper --cpuusage: {error}; /proc/stat: {fallback_error}"
                )),
            },
        },
    };
    set_capability(py, &capabilities, "system_cpu", &cpu_status)?;

    let mem_output = client.shell(&udid, "hidumper --mem", 15);
    let mem_status = match mem_output {
        Ok(text) => collector::classify_probe(
            Ok(text.clone()),
            parse_hidumper_mem_system(&text).map(|_| ()),
            "hidumper --mem",
        ),
        Err(error) => match client.shell(&udid, "cat /proc/meminfo", 15) {
            Ok(text) => collector::classify_probe(
                Ok(text.clone()),
                parse_proc_meminfo(&text).map(|_| ()),
                "cat /proc/meminfo",
            ),
            Err(fallback_error) => collector::CapabilityStatus {
                status: "unavailable".to_string(),
                reason: Some(format!(
                    "hidumper --mem: {error}; /proc/meminfo: {fallback_error}"
                )),
            },
        },
    };
    set_capability(py, &capabilities, "system_memory", &mem_status)?;

    let process_output = client.shell(&udid, "ps -ef", 15);
    let process_status = match process_output {
        Ok(text) => collector::classify_probe(
            Ok(text.clone()),
            if parse_ps_ef(&text).is_empty() {
                Err(error::Error::ParseFailed("ps -ef 未解析到进程".to_string()))
            } else {
                Ok(())
            },
            "ps -ef",
        ),
        Err(error) => collector::CapabilityStatus {
            status: "unavailable".to_string(),
            reason: Some(error.to_string()),
        },
    };
    set_capability(py, &capabilities, "process", &process_status)?;

    for (name, command, parser) in [
        ("network", collector::NETWORK_COMMAND, "network"),
        ("power", collector::POWER_COMMAND, "power"),
        ("thermal", collector::THERMAL_COMMAND, "thermal"),
        ("gpu", collector::GPU_COMMAND, "gpu"),
        ("cpu_freq", collector::CPUFREQ_COMMAND, "cpufreq"),
    ] {
        let status = match client.shell(&udid, command, 15) {
            Ok(text) => {
                let parsed = match parser {
                    "network" => parse::parse_proc_net_dev(&text).map(|_| ()),
                    "power" => collector::collect_power(&text, &mut HashMap::new()),
                    "thermal" => collector::collect_thermal(&text, &mut HashMap::new()),
                    "cpufreq" => collector::collect_cpufreq(&text, &mut HashMap::new()),
                    "gpu" => {
                        let mut sample = data::Sample {
                            sequence: 0,
                            elapsed_ms: 0,
                            system: Default::default(),
                            timestamp: chrono::Utc::now(),
                            hwinfo_raw: HashMap::new(),
                            processes: None,
                            aggregated: None,
                            top_n_cpu: None,
                            top_n_gpu: None,
                        };
                        collector::collect_gpu(&text, &mut sample)
                    }
                    _ => unreachable!(),
                };
                collector::classify_probe(Ok(text), parsed, command)
            }
            Err(error) => collector::CapabilityStatus {
                status: "unavailable".to_string(),
                reason: Some(error.to_string()),
            },
        };
        set_capability(py, &capabilities, name, &status)?;
    }
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

fn set_capability(
    py: Python<'_>,
    capabilities: &Bound<'_, PyDict>,
    name: &str,
    status: &collector::CapabilityStatus,
) -> PyResult<()> {
    capabilities.set_item(
        name,
        capability_to_py(py, &status.status, status.reason.as_deref())?,
    )?;
    Ok(())
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
    // 默认关闭 network/gpu/thermal，避免 P0 路径默认突破 4/6 次 HDC shell 预算。
    // 调用方可通过参数显式开启 optional collector。
    #[pyo3(signature = (udid, interval=2.0, duration=None, hdc_path=None, process_filter=None, top_n_cpu=10, top_n_gpu=None, enable_aggregation=true, enable_system_cpu=true, enable_system_mem=true, enable_process=true, enable_gpu=false, enable_network=false, enable_thermal_power=false, enable_cpu_freq=false, enable_disk_io=false))]
    fn new(
        udid: String,
        interval: f64,
        duration: Option<f64>,
        hdc_path: Option<String>,
        process_filter: Option<Py<PyProcessFilter>>,
        top_n_cpu: Option<usize>,
        top_n_gpu: Option<usize>,
        enable_aggregation: bool,
        enable_system_cpu: bool,
        enable_system_mem: bool,
        enable_process: bool,
        enable_gpu: bool,
        enable_network: bool,
        enable_thermal_power: bool,
        enable_cpu_freq: bool,
        enable_disk_io: bool,
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
        if top_n_cpu == Some(0) || top_n_gpu == Some(0) {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "top_n_cpu 和 top_n_gpu 必须大于 0 或为 None",
            ));
        }
        let filter = Python::with_gil(|py| {
            process_filter
                .as_ref()
                .map(|value| value.bind(py).borrow().filter.clone())
        });
        Ok(Self {
            inner: std::sync::Arc::new(MonitorInner::new()),
            config: Some(monitor::MonitorConfigOwned {
                udid,
                hdc_path,
                interval,
                duration,
                process_filter: filter,
                top_n_cpu,
                top_n_gpu,
                enable_aggregation,
                enable_system_cpu,
                enable_system_mem,
                enable_process,
                enable_gpu,
                enable_network,
                enable_thermal_power,
                enable_cpu_freq,
                enable_disk_io,
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
    m.add_class::<PyProcessFilter>()?;
    m.add_class::<PyMonitorResult>()?;
    m.add_class::<PySample>()?;
    m.add_class::<PyMonitor>()?;
    m.add_class::<TargetInfo>()?;
    m.add_function(wrap_pyfunction!(list_targets, m)?)?;
    m.add_function(wrap_pyfunction!(parse_targets_for_test, m)?)?;
    m.add_function(wrap_pyfunction!(list_processes, m)?)?;
    m.add_function(wrap_pyfunction!(list_sensors, m)?)?;
    m.add_function(wrap_pyfunction!(probe, m)?)?;
    Ok(())
}
