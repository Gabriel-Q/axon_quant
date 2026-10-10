//! KlineBuffer Python 绑定
//!
//! 把 `axon_core::KlineBuffer` 暴露到 Python，供实盘因子注入使用：
//! 启动时 `load_history` 填充历史 K 线，实时 `push_tick` 更新当前 Bar，
//! `bars` 供因子计算读取。

use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

use axon_core::market::{Bar, BarPeriod, KlineBuffer, Side, Tick};
use axon_core::time::Timestamp;
use axon_core::types::{Price, Quantity};

/// Python 端 KlineBuffer
///
/// 用法：
/// ```python
/// from axon_quant.backtest import KlineBuffer
/// buf = KlineBuffer("1h", 100)
/// buf.load_history([{"timestamp": ..., "open": ..., ...}, ...])
/// buf.push_tick(timestamp_ns=..., price=100.0)
/// bars = buf.bars()  # list[dict]
/// ```
#[pyclass(name = "KlineBuffer")]
pub struct PyKlineBuffer {
    inner: KlineBuffer,
}

#[pymethods]
impl PyKlineBuffer {
    /// 创建 KlineBuffer
    ///
    /// - `period`: K 线周期字符串（"1m", "5m", "15m", "30m", "1h", "4h", "1d"）
    /// - `maxlen`: 缓冲最大长度（已收盘 Bar 数量）
    #[new]
    fn new(period: &str, maxlen: usize) -> PyResult<Self> {
        let period = parse_period(period)?;
        Ok(Self {
            inner: KlineBuffer::new(period, maxlen),
        })
    }

    /// 加载历史 K 线（list[dict]，每个 dict 含 timestamp/open/high/low/close/volume）
    fn load_history(&mut self, bars: Bound<'_, PyList>) -> PyResult<()> {
        let history: Vec<Bar> = bars
            .iter()
            .map(|item| dict_to_bar(&item))
            .collect::<PyResult<Vec<_>>>()?;
        self.inner.load_history(history);
        Ok(())
    }

    /// 用 tick 更新当前 Bar
    ///
    /// - `timestamp_ns`: 纳秒时间戳
    /// - `price`: 成交价
    fn push_tick(&mut self, timestamp_ns: i64, price: f64) {
        let tick = Tick::new(
            Timestamp::from_nanos(timestamp_ns),
            Price::from_f64(price),
            Quantity::from_f64(0.0),
            Side::Buy,
        );
        self.inner.push_tick(&tick);
    }

    /// 获取所有 Bar（已收盘 + 当前未收盘），返回 list[dict]
    fn bars<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for bar in self.inner.bars() {
            list.append(bar_to_dict(py, &bar)?)?;
        }
        Ok(list)
    }

    /// 当前缓冲长度（含未收盘 Bar）
    fn len(&self) -> usize {
        self.inner.len()
    }

    /// 缓冲是否为空
    fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }
}

/// 解析周期字符串为 BarPeriod
fn parse_period(s: &str) -> PyResult<BarPeriod> {
    match s {
        "1m" => Ok(BarPeriod::OneMinute),
        "5m" => Ok(BarPeriod::FiveMinutes),
        "15m" => Ok(BarPeriod::FifteenMinutes),
        "30m" => Ok(BarPeriod::ThirtyMinutes),
        "1h" => Ok(BarPeriod::OneHour),
        "4h" => Ok(BarPeriod::FourHours),
        "1d" => Ok(BarPeriod::OneDay),
        other => Err(pyo3::exceptions::PyValueError::new_err(format!(
            "unsupported period: {other}, supported: 1m/5m/15m/30m/1h/4h/1d"
        ))),
    }
}

/// Python dict → Bar
fn dict_to_bar(dict: &Bound<'_, PyAny>) -> PyResult<Bar> {
    let timestamp_ns: i64 = dict.get_item("timestamp")?.extract()?;
    let open: f64 = dict.get_item("open")?.extract()?;
    let high: f64 = dict.get_item("high")?.extract()?;
    let low: f64 = dict.get_item("low")?.extract()?;
    let close: f64 = dict.get_item("close")?.extract()?;
    let volume: f64 = dict.get_item("volume")?.extract()?;

    Ok(Bar {
        timestamp: Timestamp::from_nanos(timestamp_ns),
        open: Price::from_f64(open),
        high: Price::from_f64(high),
        low: Price::from_f64(low),
        close: Price::from_f64(close),
        volume: Quantity::from_f64(volume),
    })
}

/// Bar → Python dict
fn bar_to_dict<'py>(py: Python<'py>, bar: &Bar) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("timestamp", bar.timestamp.nanos)?;
    d.set_item("open", bar.open.as_f64())?;
    d.set_item("high", bar.high.as_f64())?;
    d.set_item("low", bar.low.as_f64())?;
    d.set_item("close", bar.close.as_f64())?;
    d.set_item("volume", bar.volume.as_f64())?;
    Ok(d)
}

/// 注册 KlineBuffer 到 Python 模块
pub fn register(parent: &Bound<'_, PyModule>) -> PyResult<()> {
    parent.add_class::<PyKlineBuffer>()?;
    Ok(())
}
