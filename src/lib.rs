use std::collections::HashMap;

use pyo3::prelude::*;
use pyo3::PyTypeInfo;

mod data;
mod error;
mod hdc;
mod ring_buffer;
mod parse;

use hdc::{parse_list_targets, HdcClient};

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

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", "0.1.0")?;
    m.add("HdcError", error::HdcError::type_object_bound(m.py()))?;
    m.add_class::<PyHdcErrorKind>()?;
    m.add_class::<TargetInfo>()?;
    m.add_function(wrap_pyfunction!(list_targets, m)?)?;
    m.add_function(wrap_pyfunction!(parse_targets_for_test, m)?)?;
    Ok(())
}
