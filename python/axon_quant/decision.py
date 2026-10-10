"""axon_quant.decision — SDM 结构化决策层 Python API

封装 Rust 端 ``_native.llm.decision`` 绑定。典型用法::

    from axon_quant.decision import (
        make_decision_backend, SdmCascadeDecisionLayer, CascadeConfig, RuleFallback,
    )

    backend = make_decision_backend({"mode": "stub"})
    layer = SdmCascadeDecisionLayer(backend, RuleFallback(), CascadeConfig())
    d = layer.decide(STATE, QUESTIONS)
    print(d["action"], d["source"], d["confidence"])
"""

from __future__ import annotations

from axon_quant._native import llm as _native_llm_module

_native_decision = getattr(_native_llm_module, "decision", None)
if _native_decision is None:  # pragma: no cover - 仅在 wheel 未含 decision 时触发
    raise ImportError(
        "axon_quant._native.llm.decision 不可用：请安装含 decision 能力的 0.15.0+ wheel"
    )

make_decision_backend = _native_decision.make_decision_backend
DecisionBackend = _native_decision.DecisionBackend
RuleFallback = _native_decision.RuleFallback
CascadeConfig = _native_decision.CascadeConfig
SdmCascadeDecisionLayer = _native_decision.SdmCascadeDecisionLayer

__all__ = [
    "make_decision_backend",
    "DecisionBackend",
    "RuleFallback",
    "CascadeConfig",
    "SdmCascadeDecisionLayer",
]
