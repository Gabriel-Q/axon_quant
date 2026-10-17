//! axon-worker Python 绑定
//!
//! 暴露 BacktestResult（回测结果 DTO）+ BacktestRunnerConfig。
//! bar_nav_curve 在 Python 端转成 `Vec<(i64, f64)>`（nanos → NAV），便于 pandas/numpy 直接用。

#![cfg(feature = "python")]

use pyo3::prelude::*;

use crate::backtest::BacktestResult;

/// Python 端 BacktestResult（DTO，纯数据）
#[pyclass(name = "BacktestResult", module = "axon_worker")]
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PyBacktestResult {
    /// 已处理事件总数
    pub events_processed: u64,
    /// 接受的订单数
    pub orders_accepted: u64,
    /// 拒绝的订单数
    pub orders_rejected: u64,
    /// 成交数
    pub fills: u64,
    /// 最终 PnL（final_nav - initial_cash）
    pub total_pnl: f64,
    /// 最大回撤（USD 绝对值）
    pub max_drawdown: f64,
    /// 最终净资产
    pub final_nav: f64,
    /// 每 bar 末 mark-to-market NAV 曲线（nanos 时间戳 → NAV）
    pub bar_nav_curve: Vec<(i64, f64)>,
    /// 回测 bar 数
    pub n_bars: u64,
    /// 策略数
    pub n_strategies: u32,
}

impl From<&BacktestResult> for PyBacktestResult {
    fn from(r: &BacktestResult) -> Self {
        Self {
            events_processed: r.events_processed,
            orders_accepted: r.orders_accepted,
            orders_rejected: r.orders_rejected,
            fills: r.fills,
            total_pnl: r.total_pnl,
            max_drawdown: r.max_drawdown,
            final_nav: r.final_nav,
            // Timestamp → i64 纳秒，pandas/numpy 直接吃
            bar_nav_curve: r
                .bar_nav_curve
                .iter()
                .map(|(ts, nav)| (ts.nanos, *nav))
                .collect(),
            n_bars: r.n_bars,
            n_strategies: r.n_strategies,
        }
    }
}

#[pymethods]
impl PyBacktestResult {
    pub fn __repr__(&self) -> String {
        format!(
            "BacktestResult(bars={}, strategies={}, fills={}, final_nav={:.2}, pnl={:.2})",
            self.n_bars, self.n_strategies, self.fills, self.final_nav, self.total_pnl
        )
    }
}

/// 在给定 PyModule 上注册 axon_worker 的 Python 绑定
pub fn register_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBacktestResult>()?;
    Ok(())
}
