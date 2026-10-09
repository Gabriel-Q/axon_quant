"""axon_quant.decision 绑定集成测试（0.15.0）

验证：
1. import axon_quant.decision 成功
2. stub 后端 + 规则兜底级联返回字段齐全的 dict
3. decide_async 可 await 且结果与同步一致

运行前提：已构建并安装 axon-quant 0.15.0 wheel（maturin develop / pip install）。
运行方式：
    cd python && python tests/test_decision_binding.py
"""

from __future__ import annotations

import asyncio

from axon_quant.decision import (
    CascadeConfig,
    RuleFallback,
    SdmCascadeDecisionLayer,
    make_decision_backend,
)

# 强多头状态（sma 与 ret 同向），stub 大概率给 buy
STATE = {
    "features": {
        "ret_1": 0.012,
        "ret_5": 0.03,
        "vol_20": 0.008,
        "sma5_vs_sma20": 0.02,
        "volume_z": 0.3,
    }
}

# 仅提供主问题即可（stub 不读 questions，但反序列化要求类型合法）
QUESTIONS = {
    "next_direction": {
        "type": "choice",
        "instructions": {"question": "下一根方向？", "focus": "仅依据给定数据"},
        "criteria": {
            "buy": {"what": "涨"},
            "sell": {"what": "跌"},
            "hold": {"what": "震荡"},
        },
    }
}


def check_decision(d: dict) -> None:
    for key in ("action", "source", "confidence", "probabilities",
                "fallback_reason", "sdm_usage", "sub_signals"):
        assert key in d, f"missing key {key}: {d}"
    assert d["action"] in ("buy", "sell", "hold")
    assert d["source"] in ("sdm_direct", "fallback")
    for k in ("buy", "sell", "hold"):
        assert k in d["probabilities"], d["probabilities"]


def test_sync_decide() -> dict:
    backend = make_decision_backend({"mode": "stub"})
    layer = SdmCascadeDecisionLayer(backend, RuleFallback(), CascadeConfig())
    d = layer.decide(STATE, QUESTIONS)
    check_decision(d)
    print("sync:", d["action"], d["source"], d["confidence"])
    return d


async def test_async_decide() -> None:
    backend = make_decision_backend({"mode": "stub"})
    layer = SdmCascadeDecisionLayer(backend, RuleFallback(), CascadeConfig())
    d = await layer.decide_async(STATE, QUESTIONS)
    check_decision(d)
    print("async:", d["action"], d["source"], d["confidence"])


def main() -> None:
    test_sync_decide()
    asyncio.run(test_async_decide())
    print("OK")


if __name__ == "__main__":
    main()
