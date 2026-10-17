//! axon-worker Python 绑定（骨架级）
//!
//! 暴露 BacktestResult（回测结果 DTO）+ StrategyManager（策略注册入口）。

#![cfg(feature = "python")]

use pyo3::prelude::*;

use crate::backtest::BacktestResult;
use crate::manager::StrategyManager;

/// Python 端 BacktestResult（DTO，纯数据）
#[pyclass(name = "BacktestResult", module = "axon_worker")]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PyBacktestResult {
    pub events_processed: u64,
    pub orders_accepted: u64,
    pub orders_rejected: u64,
    pub n_bars: u64,
    pub n_strategies: u32,
}

impl From<&BacktestResult> for PyBacktestResult {
    fn from(r: &BacktestResult) -> Self {
        Self {
            events_processed: r.events_processed,
            orders_accepted: r.orders_accepted,
            orders_rejected: r.orders_rejected,
            n_bars: r.n_bars,
            n_strategies: r.n_strategies,
        }
    }
}

#[pymethods]
impl PyBacktestResult {
    pub fn __repr__(&self) -> String {
        format!(
            "BacktestResult(bars={}, strategies={}, events={})",
            self.n_bars, self.n_strategies, self.events_processed
        )
    }
}

/// 在给定 PyModule 上注册 axon_worker 的 Python 绑定
pub fn register_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBacktestResult>()?;
    Ok(())
}
