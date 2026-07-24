use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SensorValue {
    pub value: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub name: String,
    pub cpu_percent: f64,
    pub working_set_mb: f64,
    pub committed_memory_mb: f64,
    pub gpu_percent: f64,
    pub gpu_memory_mb: f64,
    pub handle_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedProcessInfo {
    pub name: String,
    pub pids: Vec<u32>,
    pub cpu_percent_total: f64,
    pub working_set_mb_total: f64,
    pub committed_memory_mb_total: f64,
    pub gpu_percent_total: f64,
    pub handle_count_total: u32,
    pub process_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuAdapterMetrics {
    pub luid: String,
    pub name: String,
    pub utilization_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMetrics {
    pub cpu_percent: Option<f64>,
    pub gpu_percent: Option<f64>,
    pub gpu_adapters: Vec<GpuAdapterMetrics>,
    pub gpu_source: String,
}

impl Default for SystemMetrics {
    fn default() -> Self {
        Self {
            cpu_percent: None,
            gpu_percent: None,
            gpu_adapters: Vec::new(),
            gpu_source: "unavailable".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Sample {
    pub sequence: u64,
    pub elapsed_ms: u64,
    pub system: SystemMetrics,
    pub timestamp: DateTime<Utc>,
    pub hwinfo_raw: HashMap<String, SensorValue>,
    pub processes: Option<Vec<ProcessInfo>>,
    pub aggregated: Option<Vec<AggregatedProcessInfo>>,
    pub top_n_cpu: Option<Vec<AggregatedProcessInfo>>,
    pub top_n_gpu: Option<Vec<AggregatedProcessInfo>>,
}

#[derive(Debug, Clone)]
pub enum ProcessFilter {
    Pids(Vec<u32>),
    Name(String),
    Names(Vec<String>),
    NameRegex(String),
}

#[derive(Debug, Clone)]
pub struct MonitorConfig {
    pub udid: String,
    pub hdc_path: Option<String>,
    pub interval: f64,
    pub duration: Option<f64>,
    pub process_filter: Option<ProcessFilter>,
    pub top_n_cpu: Option<usize>,
    pub top_n_gpu: Option<usize>,
    pub enable_aggregation: bool,
    pub enable_system_cpu: bool,
    pub enable_system_mem: bool,
    pub enable_process: bool,
    pub enable_gpu: bool,
    pub enable_network: bool,
    pub enable_thermal_power: bool,
}
