"""BacktestRunner.run_multi 绑定层行为验证（共享资金池路径）。"""

from axon_quant import worker


def _bars(start: float, step: float, n: int):
    return [
        {
            "timestamp": i * 60_000_000_000,
            "open": start + i * step - 0.5,
            "high": start + i * step + 0.3,
            "low": start + i * step - 0.8,
            "close": start + i * step,
            "volume": 1000.0,
        }
        for i in range(n)
    ]


class AlternatingStrategy:
    """鸭子类型策略：逐 bar 交替买卖，保证产生 round-trip 平仓成交。"""

    def __init__(self, symbol: str):
        self.symbol = symbol
        self._n = 0

    def on_bar(self, bar):
        self._n += 1
        if self._n % 2 == 1:
            return {"action_type": "buy", "confidence": 0.8, "target_position": 100.0}
        return {"action_type": "sell", "confidence": 0.8, "target_position": -100.0}


def test_run_multi_aligned_bars_returns_result():
    n = 6
    bars_by_symbol = {
        "BTC/USDT": _bars(100.0, 1.0, n),
        "ETH/USDT": _bars(20.0, 0.5, n),
    }
    runner = worker.BacktestRunner(initial_cash=100_000.0, force_liquidate=True)
    runner.add_strategy(AlternatingStrategy("BTC/USDT"), symbol="BTC/USDT")
    runner.add_strategy(AlternatingStrategy("ETH/USDT"), symbol="ETH/USDT")

    result = runner.run_multi(bars_by_symbol=bars_by_symbol)
    assert result.n_bars == n
    assert result.n_strategies == 2


def test_run_multi_none_slot_supported():
    n = 6
    bars_by_symbol = {
        "BTC/USDT": _bars(100.0, 1.0, n),
        # ETH 前两根无数据 → None 占位（价格由 Rust 侧前向填充）
        "ETH/USDT": [None, None] + _bars(20.0, 0.5, n)[2:],
    }
    runner = worker.BacktestRunner(initial_cash=100_000.0, force_liquidate=True)
    runner.add_strategy(AlternatingStrategy("BTC/USDT"), symbol="BTC/USDT")
    runner.add_strategy(AlternatingStrategy("ETH/USDT"), symbol="ETH/USDT")

    result = runner.run_multi(bars_by_symbol=bars_by_symbol)
    assert result.n_bars == n


def test_trades_expose_instrument():
    """共享模式按品种拆分成交依赖 trades 的 instrument 字段。

    force_liquidate=True 保证 EOD 平仓产生 round-trip 记录。
    """
    n = 6
    runner = worker.BacktestRunner(initial_cash=100_000.0, force_liquidate=True)
    runner.add_strategy(AlternatingStrategy("BTC/USDT"), symbol="BTC/USDT")

    result = runner.run(bars=_bars(100.0, 1.0, n))

    # 每个 fill 明细必然带 instrument（既有行为，作为兜底断言）
    for fr in result.fills_detail:
        assert "instrument" in fr

    assert len(result.trades) > 0, (
        "expected round-trip trades (alternating buy/sell + EOD liquidate); "
        "若仍为空，改为断言 fills_detail 的 instrument 字段并在此注明"
    )
    for tr in result.trades:
        assert "instrument" in tr
        assert tr["instrument"][1] == "BTC"
        assert tr["instrument"][2] == "USDT"
