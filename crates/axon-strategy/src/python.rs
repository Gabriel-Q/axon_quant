//! axon-strategy Python 绑定
//!
//! 暴露内置策略构造器（RuleStrategyImpl 等）。
//! 策略实例以 `Arc<dyn Strategy>` 持有，可直接传给 `axon_worker.BacktestRunner.add_strategy`。

#![cfg(feature = "python")]

use std::sync::Arc;

use pyo3::prelude::*;

use crate::impl_::RuleStrategyImpl;
use crate::trait_::Strategy;

/// Python 端 RuleStrategyImpl（双均线规则策略）
///
/// 持有 `Arc<dyn Strategy>`，可直接传给 BacktestRunner.add_strategy。
#[pyclass(name = "RuleStrategyImpl", module = "axon_strategy", from_py_object)]
#[derive(Clone)]
pub struct PyRuleStrategyImpl {
    /// 持有 `Arc<dyn Strategy>`，供 BacktestRunner.add_strategy 提取
    pub inner: Arc<dyn Strategy>,
}

#[pymethods]
impl PyRuleStrategyImpl {
    /// 构造：双均线策略（name + symbol）
    #[new]
    #[pyo3(signature = (name, symbol))]
    fn new(name: String, symbol: String) -> Self {
        Self {
            inner: Arc::new(RuleStrategyImpl::new(name, symbol)),
        }
    }

    fn __repr__(&self) -> String {
        format!(
            "RuleStrategyImpl(name={}, symbol={})",
            self.inner.name(),
            self.inner.symbol()
        )
    }
}

/// 在给定 PyModule 上注册 axon_strategy 的 Python 绑定
pub fn register_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyRuleStrategyImpl>()?;
    Ok(())
}
