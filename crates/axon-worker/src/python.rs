//! axon-worker Python 绑定
//!
//! 暴露：
//! - `BacktestRunner`：Rust 原生回测编排器（构造 + 注册策略 + run）
//! - `BacktestResult`：回测结果 DTO
//!
//! `run()` 内部用 tokio runtime `block_on` 执行 async 回测循环，
//! bars 从 Python `list[dict]` 构造（每根 bar 含 timestamp/open/high/low/close/volume）。

#![cfg(feature = "python")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList, PyTuple};

use axon_core::market::bar::Bar;
use axon_core::portfolio::{FillRecord, TradeRecord};
use axon_core::time::Timestamp;
use axon_core::types::{Instrument, Price, Quantity, SwapSettle};
use axon_strategy::impl_::PythonStrategyImpl;
use axon_strategy::python::PyRuleStrategyImpl;
use axon_strategy::trait_::Strategy;

use crate::backtest::{BacktestResult, BacktestRunner};

/// Python 端 BacktestResult（DTO，纯数据）
///
/// 字段与 `axon_backtest::RunResult`（Python 暴露的 `RunResult`）对齐，
/// QuantCell 可复用 `_convert_backtest_result` 的读取契约。
#[pyclass(name = "BacktestResult", module = "axon_worker", skip_from_py_object)]
#[derive(Clone, Debug)]
pub struct PyBacktestResult {
    #[pyo3(get)]
    pub events_processed: u64,
    #[pyo3(get)]
    pub orders_accepted: u64,
    #[pyo3(get)]
    pub orders_rejected: u64,
    #[pyo3(get)]
    pub orders_cancelled: u64,
    #[pyo3(get)]
    pub orders_modified: u64,
    #[pyo3(get)]
    pub fills: u64,
    #[pyo3(get)]
    pub total_pnl: f64,
    #[pyo3(get)]
    pub max_drawdown: f64,
    #[pyo3(get)]
    pub max_drawdown_pct: f64,
    #[pyo3(get)]
    pub nav_peak: f64,
    #[pyo3(get)]
    pub final_nav: f64,
    #[pyo3(get)]
    pub total_fees: f64,
    #[pyo3(get)]
    pub total_funding_pnl: f64,
    #[pyo3(get)]
    pub win_rate: f64,
    #[pyo3(get)]
    pub sharpe_ratio: f64,
    #[pyo3(get)]
    pub duration_secs: f64,
    #[pyo3(get)]
    pub final_time_ns: i64,
    #[pyo3(get)]
    pub rebalances_triggered: u64,
    #[pyo3(get)]
    pub n_bars: u64,
    #[pyo3(get)]
    pub n_strategies: u32,
    /// 每 bar 末 NAV 曲线（`[(timestamp_ns, nav), ...]`）
    #[pyo3(get)]
    pub bar_nav_curve: Vec<(i64, f64)>,
    /// 稀疏 NAV 曲线（`[(timestamp_ns, nav), ...]`）
    #[pyo3(get)]
    pub equity_curve: Vec<(i64, f64)>,
    /// round-trip 交易记录（自定义 getter → list[dict]）
    pub trades: Vec<TradeRecord>,
    /// 每笔 fill 明细（自定义 getter → list[dict]）
    pub fills_detail: Vec<FillRecord>,
    /// 终态持仓（自定义 getter → dict[tuple, float]）
    pub final_positions: Vec<(Instrument, f64)>,
}

impl From<&BacktestResult> for PyBacktestResult {
    fn from(r: &BacktestResult) -> Self {
        Self {
            events_processed: r.events_processed,
            orders_accepted: r.orders_accepted,
            orders_rejected: r.orders_rejected,
            orders_cancelled: r.orders_cancelled,
            orders_modified: r.orders_modified,
            fills: r.fills,
            total_pnl: r.total_pnl,
            max_drawdown: r.max_drawdown,
            max_drawdown_pct: r.max_drawdown_pct,
            nav_peak: r.nav_peak,
            final_nav: r.final_nav,
            total_fees: r.total_fees,
            total_funding_pnl: r.total_funding_pnl,
            win_rate: r.win_rate,
            sharpe_ratio: r.sharpe_ratio,
            duration_secs: r.duration_secs,
            final_time_ns: r.final_time_ns,
            rebalances_triggered: r.rebalances_triggered,
            n_bars: r.n_bars,
            n_strategies: r.n_strategies,
            bar_nav_curve: r
                .bar_nav_curve
                .iter()
                .map(|(ts, nav)| (ts.nanos, *nav))
                .collect(),
            equity_curve: r
                .equity_curve
                .iter()
                .map(|(ts, nav)| (ts.nanos, *nav))
                .collect(),
            trades: r.trades.clone(),
            fills_detail: r.fills_detail.clone(),
            final_positions: r.final_positions.clone(),
        }
    }
}

#[pymethods]
impl PyBacktestResult {
    /// 完整交易记录（`list[dict]`，字段与 Python 端 `RunResult.trades` 一致）
    #[getter]
    fn trades<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for tr in &self.trades {
            let d = PyDict::new(py);
            d.set_item("timestamp_ns", tr.trade.timestamp.nanos)?;
            d.set_item("price", tr.trade.price.as_f64())?;
            d.set_item("quantity", tr.trade.quantity.as_f64())?;
            d.set_item("buyer_order_id", tr.trade.buyer_order_id)?;
            d.set_item("seller_order_id", tr.trade.seller_order_id)?;
            d.set_item("realized_pnl", tr.realized_pnl as f64 / 1e6)?;
            d.set_item("commission", tr.commission as f64 / 1e6)?;
            d.set_item("net_quantity", tr.net_quantity as f64 / 1e6)?;
            d.set_item("entry_price", tr.entry_price as f64 / 1e6)?;
            d.set_item("entry_time_ns", tr.entry_time_ns)?;
            // 共享资金池模式需按品种拆分成交，故补 instrument（同 fills_detail 口径）
            d.set_item("instrument", instrument_to_tuple(py, &tr.instrument)?)?;
            list.append(d)?;
        }
        Ok(list)
    }

    /// 每笔 fill 明细（`list[dict]`）
    #[getter]
    fn fills_detail<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for fr in &self.fills_detail {
            let d = PyDict::new(py);
            d.set_item("timestamp_ns", fr.timestamp.nanos)?;
            d.set_item("instrument", instrument_to_tuple(py, &fr.instrument)?)?;
            d.set_item("taker_order_id", fr.taker_order_id)?;
            d.set_item("maker_order_id", fr.maker_order_id)?;
            d.set_item(
                "taker_side",
                match fr.taker_side {
                    axon_core::market::Side::Buy => "Buy",
                    axon_core::market::Side::Sell => "Sell",
                },
            )?;
            d.set_item("price", fr.price.as_f64())?;
            d.set_item("quantity", fr.quantity.as_f64())?;
            list.append(d)?;
        }
        Ok(list)
    }

    /// 终态持仓快照（`dict[tuple, float]`，与 Python 端 `RunResult.positions` 一致）
    #[getter]
    fn final_positions<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        for (inst, qty) in &self.final_positions {
            d.set_item(instrument_to_tuple(py, inst)?, *qty)?;
        }
        Ok(d)
    }

    pub fn __repr__(&self) -> String {
        format!(
            "BacktestResult(bars={}, strategies={}, fills={}, final_nav={:.2}, pnl={:.2})",
            self.n_bars, self.n_strategies, self.fills, self.final_nav, self.total_pnl
        )
    }
}

/// Instrument → Python tuple（与 axon-backtest 的 `instrument_to_tuple` 对齐）
///
/// spot: `("spot", base, quote)`；swap: `("swap", base, quote, settle, contract_size)`
fn instrument_to_tuple<'py>(py: Python<'py>, inst: &Instrument) -> PyResult<Bound<'py, PyTuple>> {
    match inst {
        Instrument::Spot(s) => PyTuple::new(
            py,
            [
                "spot".into_pyobject(py)?.into_any(),
                s.base.as_str().into_pyobject(py)?.into_any(),
                s.quote.as_str().into_pyobject(py)?.into_any(),
            ],
        ),
        Instrument::Swap(s) => {
            let settle = match s.settle {
                axon_core::types::SwapSettle::UsdMargin => "usd_margin",
                axon_core::types::SwapSettle::CoinMargin => "coin_margin",
            };
            PyTuple::new(
                py,
                [
                    "swap".into_pyobject(py)?.into_any(),
                    s.base.as_str().into_pyobject(py)?.into_any(),
                    s.quote.as_str().into_pyobject(py)?.into_any(),
                    settle.into_pyobject(py)?.into_any(),
                    s.contract_size.into_pyobject(py)?.into_any(),
                ],
            )
        }
    }
}

/// Python 端 BacktestRunner（Rust 原生回测编排器）
///
/// 用法：
/// ```python
/// from axon_quant import strategy, worker
/// runner = worker.BacktestRunner(initial_cash=100000.0)
/// runner.add_strategy(strategy.RuleStrategyImpl("ma", "BTCUSDT"))
/// result = runner.run(bars=[{"timestamp": ns, "open":..., ...}, ...], state_by_symbol={})
/// ```
#[pyclass(name = "BacktestRunner", module = "axon_worker")]
pub struct PyBacktestRunner {
    inner: Mutex<BacktestRunner>,
    runtime: tokio::runtime::Runtime,
}

#[pymethods]
impl PyBacktestRunner {
    #[new]
    #[pyo3(signature = (initial_cash=100000.0, futures=false, force_liquidate=false, settle=None, contract_size=1.0))]
    fn new(
        initial_cash: f64,
        futures: bool,
        force_liquidate: bool,
        settle: Option<String>,
        contract_size: f64,
    ) -> PyResult<Self> {
        let runtime = tokio::runtime::Runtime::new().map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("创建 tokio runtime 失败: {e}"))
        })?;
        let mut runner = BacktestRunner::simple(initial_cash);
        if futures {
            // 保证金结算方式：`usd_margin`（U 本位，默认）/ `coin_margin`（币本位）；
            // 合约乘数默认 1.0。注意 `contract_size` 目前只是品种元数据 ——
            // axon-backtest 引擎尚未把乘数计入名义价值/盈亏（engine.rs 里挂着 0.8.0 待办），
            // 币本位只有结算方式被如实标注，账务仍是近似。
            let settle = match settle.as_deref().unwrap_or("usd_margin") {
                "usd_margin" => SwapSettle::UsdMargin,
                "coin_margin" => SwapSettle::CoinMargin,
                other => {
                    return Err(pyo3::exceptions::PyValueError::new_err(format!(
                        "unknown settle: {other} (expected 'usd_margin' / 'coin_margin')"
                    )));
                }
            };
            runner = runner.with_futures(settle, contract_size);
        }
        // 回测结束 EOD 强制平仓
        runner = runner.with_force_liquidate(force_liquidate);
        Ok(Self {
            inner: Mutex::new(runner),
            runtime,
        })
    }

    /// 注册一个策略
    ///
    /// 支持两种类型：
    /// - `strategy.RuleStrategyImpl` 实例（Rust 内置双均线策略）
    /// - 任意 Python 对象，需实现 `decide(bar_dict)` 或 `on_bar(bar_dict)` 方法，
    ///   返回含 `action_type`/`confidence`/`target_position` 的 dict 或对象
    #[pyo3(signature = (strategy, name=None, symbol=None))]
    fn add_strategy(
        &self,
        strategy: &Bound<'_, PyAny>,
        name: Option<String>,
        symbol: Option<String>,
    ) -> PyResult<()> {
        let s: Arc<dyn Strategy> =
            if let Ok(py_rule) = strategy.extract::<PyRef<'_, PyRuleStrategyImpl>>() {
                // Rust 内置策略：直接提取 Arc<dyn Strategy>
                py_rule.inner.clone()
            } else {
                // Python 自定义策略：用 PythonStrategyImpl 包装
                let name = name.unwrap_or_else(|| {
                    strategy
                        .getattr("name")
                        .and_then(|v| v.extract::<String>())
                        .unwrap_or_else(|_| "python_strategy".to_string())
                });
                let symbol = symbol.unwrap_or_else(|| {
                    strategy
                        .getattr("symbol")
                        .and_then(|v| v.extract::<String>())
                        .unwrap_or_else(|_| "UNKNOWN".to_string())
                });
                Arc::new(PythonStrategyImpl::new(
                    name,
                    symbol,
                    strategy.clone().unbind(),
                ))
            };

        self.inner
            .lock()
            .map_err(|e| {
                pyo3::exceptions::PyRuntimeError::new_err(format!("BacktestRunner lock 失败: {e}"))
            })?
            .add_strategy(s);
        Ok(())
    }

    /// 执行回测（同步阻塞，内部 block_on async 循环）
    ///
    /// 单品种与多品种共享账户统一走这一条路径：单品种只需传一个 symbol 的完整
    /// bar 列表（无 `None` 缺口）。
    ///
    /// - `bars_by_symbol`: `dict[str, list[dict | None]]`，各 symbol 的**等长**逐网格点
    ///   bar 列表；`None` 表示该网格点无真实 bar（价格由 Rust 侧前向填充参与盯市，
    ///   但不触发策略决策）
    /// - `states_by_symbol`: dict[str, dict | list[dict]]；值为 list 时按**逐 bar** 注入
    ///   （第 i 项对应第 i 个网格点），值为单对象时展开为「每 bar 同值」的便捷用法
    /// - `funding_by_symbol`: dict[str, dict[int, tuple(rate, mark_price)]]，资金费查找表
    ///   （键为 bar 纳秒时间戳；命中即入队结算）。现货回测传 None 即可
    /// - `capital_budget`: `float | None`，资金约束预算（`initial_cash × leverage`）；
    ///   `None` 表示不启用约束。单品种回测传 None
    #[pyo3(signature = (bars_by_symbol, states_by_symbol=None, funding_by_symbol=None, capital_budget=None))]
    fn run(
        &self,
        bars_by_symbol: &Bound<'_, PyDict>,
        states_by_symbol: Option<Bound<'_, PyDict>>,
        funding_by_symbol: Option<Bound<'_, PyDict>>,
        capital_budget: Option<f64>,
    ) -> PyResult<PyBacktestResult> {
        // 1) dict[str, list[dict|None]] → BTreeMap<String, Vec<Option<Bar>>>
        let mut bars_map: BTreeMap<String, Vec<Option<Bar>>> = BTreeMap::new();
        for (k, v) in bars_by_symbol.iter() {
            let key: String = k.extract()?;
            let items: Vec<Bound<'_, PyAny>> = v.extract()?;
            let mut per_grid: Vec<Option<Bar>> = Vec::with_capacity(items.len());
            for it in items.iter() {
                if it.is_none() {
                    per_grid.push(None);
                } else {
                    per_grid.push(Some(dict_to_bar(it.cast::<PyDict>()?)?));
                }
            }
            bars_map.insert(key, per_grid);
        }
        if bars_map.is_empty() {
            return Err(pyo3::exceptions::PyValueError::new_err(
                "bars_by_symbol 不能为空",
            ));
        }
        let grid_len = bars_map.values().map(|v| v.len()).max().unwrap_or(0);

        let py = bars_by_symbol.py();
        let json_mod = py.import("json")?;

        // 2) states_by_symbol（逐 bar；长度不齐时按网格长度补齐「每 bar 同值」）
        let states: BTreeMap<String, Vec<serde_json::Value>> = match states_by_symbol {
            Some(d) => {
                let mut m = BTreeMap::new();
                for (k, v) in d.iter() {
                    let key: String = k.extract()?;
                    let list = if v.is_instance_of::<PyList>() {
                        let items: Vec<Bound<'_, PyAny>> = v.extract()?;
                        items
                            .iter()
                            .map(|it| dict_to_json_value(&json_mod, it))
                            .collect::<PyResult<Vec<_>>>()?
                    } else {
                        vec![dict_to_json_value(&json_mod, &v)?; grid_len]
                    };
                    m.insert(key, list);
                }
                m
            }
            None => BTreeMap::new(),
        };

        // 3) funding_by_symbol → BTreeMap<String, BTreeMap<i64, (f64, f64)>>
        let funding: BTreeMap<String, BTreeMap<i64, (f64, f64)>> = match funding_by_symbol {
            Some(d) => {
                let mut m = BTreeMap::new();
                for (k, v) in d.iter() {
                    let key: String = k.extract()?;
                    let sched: BTreeMap<i64, (f64, f64)> = v.extract()?;
                    m.insert(key, sched);
                }
                m
            }
            None => BTreeMap::new(),
        };

        // 4) block_on 执行 async 回测
        let runner = self.inner.lock().map_err(|e| {
            pyo3::exceptions::PyRuntimeError::new_err(format!("BacktestRunner lock 失败: {e}"))
        })?;
        let result =
            self.runtime
                .block_on(runner.run(&bars_map, &states, &funding, capital_budget));

        Ok((&result).into())
    }
}

/// Python 对象 → serde_json::Value（经 json.dumps，避免 str() 的单引号问题）
fn dict_to_json_value(
    json_mod: &Bound<'_, PyAny>,
    v: &Bound<'_, PyAny>,
) -> PyResult<serde_json::Value> {
    let json_str: String = json_mod.call_method1("dumps", (v,))?.extract()?;
    Ok(serde_json::from_str(&json_str).unwrap_or(serde_json::Value::Null))
}

/// 从 Python dict 构造 Bar
fn dict_to_bar(d: &Bound<'_, PyDict>) -> PyResult<Bar> {
    let ts_nanos: i64 = d
        .get_item("timestamp")?
        .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("bar 缺少 timestamp"))?
        .extract()?;
    let open: f64 = d
        .get_item("open")?
        .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("bar 缺少 open"))?
        .extract()?;
    let high: f64 = d
        .get_item("high")?
        .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("bar 缺少 high"))?
        .extract()?;
    let low: f64 = d
        .get_item("low")?
        .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("bar 缺少 low"))?
        .extract()?;
    let close: f64 = d
        .get_item("close")?
        .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("bar 缺少 close"))?
        .extract()?;
    let volume: f64 = d
        .get_item("volume")?
        .ok_or_else(|| pyo3::exceptions::PyKeyError::new_err("bar 缺少 volume"))?
        .extract()?;

    Ok(Bar {
        timestamp: Timestamp { nanos: ts_nanos },
        open: Price::from_f64(open),
        high: Price::from_f64(high),
        low: Price::from_f64(low),
        close: Price::from_f64(close),
        volume: Quantity::from_f64(volume),
    })
}

/// 在给定 PyModule 上注册 axon_worker 的 Python 绑定
pub fn register_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBacktestResult>()?;
    m.add_class::<PyBacktestRunner>()?;
    Ok(())
}
