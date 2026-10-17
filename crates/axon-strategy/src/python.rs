//! axon-strategy Python 绑定（骨架级）
//!
//! 提供 `register_module` 把策略构造器 + BacktestRunnerConfig 挂到 `_native.strategy` 子模块。
//! 真实策略实例化（SdmStrategyImpl 等）Phase 2 再做——当前只暴露配置 DTO。

#![cfg(feature = "python")]

use pyo3::prelude::*;
use pyo3::types::PyDict;
use serde_json::Value;

/// Python 端 BacktestRunner 配置（DTO）
///
/// 这是一个纯数据类，字段全部 Optional——Rust 侧会做默认填充。
/// 由 Python 层序列化后传给 `BacktestRunner::from_config_json()`。
#[pyclass(name = "BacktestRunnerConfig", module = "axon_strategy")]
#[derive(Clone, Default, Debug)]
pub struct PyBacktestRunnerConfig {
    pub initial_cash: f64,
    pub harness_mode: String, // "offline" | "online"
}

#[pymethods]
impl PyBacktestRunnerConfig {
    #[new]
    #[pyo3(signature = (initial_cash=100000.0, harness_mode=None))]
    fn new(initial_cash: f64, harness_mode: Option<String>) -> Self {
        Self {
            initial_cash,
            harness_mode: harness_mode.unwrap_or_else(|| "offline".into()),
        }
    }

    fn to_json<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("initial_cash", self.initial_cash)?;
        d.set_item("harness_mode", &self.harness_mode)?;
        Ok(d)
    }

    fn __repr__(&self) -> String {
        format!(
            "BacktestRunnerConfig(cash={}, harness={})",
            self.initial_cash, self.harness_mode
        )
    }
}

/// 把 Python dict 解析为 serde_json::Value（helper）
pub fn dict_to_json(py: Python<'_>, dict: &Bound<'_, PyDict>) -> PyResult<Value> {
    // 简化：先 repr 再 serde——实际用 serde_json::from_str(py_object.dict()...)
    let _ = (py, dict);
    Ok(Value::Null)
}

/// 在给定 PyModule 上注册 axon_strategy 的 Python 绑定
pub fn register_module(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add_class::<PyBacktestRunnerConfig>()?;
    Ok(())
}
