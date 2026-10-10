//! 滚动 K 线缓冲（KlineBuffer）
//!
//! 实盘因子注入的基础设施：维护最近 N 根 K 线，收到 tick 时更新当前未收盘 Bar，
//! 跨周期时收盘旧 Bar 并追加新 Bar。供因子计算读取历史 OHLCV。
//!
//! ponytail: `volume` 暂不支持从 tick 精确更新（Ticker 只有 24h volume，
//!           无法还原单根 K 线成交量），价量因子会读到 0。后续用 WebSocket
//!           trade 流累计 volume 补齐。

use std::collections::VecDeque;

use super::{Bar, BarPeriod, Tick};
use crate::time::Timestamp;
use crate::types::Quantity;

/// 滚动 K 线缓冲
///
/// - 启动时通过 [`KlineBuffer::load_history`] 填充历史 K 线（来自 fetch_ohlcv）
/// - 实时通过 [`KlineBuffer::push_tick`] 更新当前未收盘 Bar 的 OHLC
/// - 检测到新 K 线开盘时，收盘旧 Bar 并追加新 Bar
/// - 通过 [`KlineBuffer::bars`] 供指标/因子计算读取
#[derive(Debug, Clone)]
pub struct KlineBuffer {
    period: BarPeriod,
    bars: VecDeque<Bar>,
    maxlen: usize,
    /// 当前未收盘 Bar（None 表示缓冲为空，等待第一个 tick）
    current: Option<Bar>,
}

impl KlineBuffer {
    /// 创建新的 KlineBuffer
    ///
    /// - `period`: K 线周期
    /// - `maxlen`: 缓冲最大长度（已收盘 Bar 数量，不含当前未收盘 Bar）
    pub fn new(period: BarPeriod, maxlen: usize) -> Self {
        Self {
            period,
            bars: VecDeque::with_capacity(maxlen),
            maxlen,
            current: None,
        }
    }

    /// 启动时加载历史 K 线（来自 fetch_ohlcv）
    ///
    /// 超出 maxlen 的最旧 Bar 会被丢弃。
    pub fn load_history(&mut self, history: Vec<Bar>) {
        for bar in history {
            self.push_closed(bar);
        }
    }

    /// 用 tick 更新当前 Bar；跨周期时收盘旧 Bar 并开新 Bar
    pub fn push_tick(&mut self, tick: &Tick) {
        let period_nanos = self.period.nanos();
        // 当前 tick 所属 K 线的起始时间戳（向下取整到周期边界）
        let bar_start_ns = tick.timestamp.nanos - (tick.timestamp.nanos % period_nanos);
        let bar_ts = Timestamp::from_nanos(bar_start_ns);

        match self.current.as_mut() {
            Some(cur) if cur.timestamp == bar_ts => {
                // 同一根 K 线：更新 high/low/close
                let price = tick.price;
                if price.as_f64() > cur.high.as_f64() {
                    cur.high = price;
                }
                if price.as_f64() < cur.low.as_f64() {
                    cur.low = price;
                }
                cur.close = price;
                // volume 暂不更新（见 ponytail 注释）
            }
            _ => {
                // 新 K 线：先把 current 收盘入队，再开新 Bar
                if let Some(closed) = self.current.take() {
                    self.push_closed(closed);
                }
                self.current = Some(Bar {
                    timestamp: bar_ts,
                    open: tick.price,
                    high: tick.price,
                    low: tick.price,
                    close: tick.price,
                    volume: Quantity::from_f64(0.0), // 待 trade 流补齐
                });
            }
        }
    }

    /// 获取所有已收盘 Bar + 当前未收盘 Bar（供因子计算）
    pub fn bars(&self) -> Vec<Bar> {
        let mut v: Vec<Bar> = self.bars.iter().copied().collect();
        if let Some(cur) = self.current {
            v.push(cur);
        }
        v
    }

    /// 当前缓冲长度（含未收盘 Bar）
    pub fn len(&self) -> usize {
        self.bars.len() + if self.current.is_some() { 1 } else { 0 }
    }

    /// 缓冲是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 入队一根已收盘 Bar，超过 maxlen 时弹出最旧的
    fn push_closed(&mut self, bar: Bar) {
        self.bars.push_back(bar);
        if self.bars.len() > self.maxlen {
            self.bars.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::market::Side;
    use crate::types::Price;

    fn make_tick(nanos: i64, price: f64) -> Tick {
        Tick::new(
            Timestamp::from_nanos(nanos),
            Price::from_f64(price),
            Quantity::from_f64(0.0),
            Side::Buy,
        )
    }

    fn make_bar(ts_nanos: i64, open: f64, high: f64, low: f64, close: f64) -> Bar {
        Bar {
            timestamp: Timestamp::from_nanos(ts_nanos),
            open: Price::from_f64(open),
            high: Price::from_f64(high),
            low: Price::from_f64(low),
            close: Price::from_f64(close),
            volume: Quantity::from_f64(0.0),
        }
    }

    #[test]
    fn test_push_tick_same_period_updates_ohlc() {
        // 1 分钟周期，第 0 秒开盘
        let mut buf = KlineBuffer::new(BarPeriod::OneMinute, 10);
        // 12:00:00 开盘价 100
        buf.push_tick(&make_tick(0, 100.0));
        // 12:00:30 最高价 105
        buf.push_tick(&make_tick(30 * 1_000_000_000, 105.0));
        // 12:00:45 最低价 98，收盘 98
        buf.push_tick(&make_tick(45 * 1_000_000_000, 98.0));

        let bars = buf.bars();
        assert_eq!(bars.len(), 1);
        let cur = bars[0];
        assert_eq!(cur.open.as_f64(), 100.0);
        assert_eq!(cur.high.as_f64(), 105.0);
        assert_eq!(cur.low.as_f64(), 98.0);
        assert_eq!(cur.close.as_f64(), 98.0);
    }

    #[test]
    fn test_push_tick_cross_period_closes_old_bar() {
        let mut buf = KlineBuffer::new(BarPeriod::OneMinute, 10);
        // 第 1 分钟：100 → 105
        buf.push_tick(&make_tick(0, 100.0));
        buf.push_tick(&make_tick(30 * 1_000_000_000, 105.0));
        // 第 2 分钟：跨周期，旧 Bar 收盘
        buf.push_tick(&make_tick(60 * 1_000_000_000, 110.0));

        let bars = buf.bars();
        assert_eq!(bars.len(), 2);
        // 已收盘的第一根
        assert_eq!(bars[0].open.as_f64(), 100.0);
        assert_eq!(bars[0].close.as_f64(), 105.0);
        // 当前未收盘的第二根
        assert_eq!(bars[1].open.as_f64(), 110.0);
        assert_eq!(bars[1].close.as_f64(), 110.0);
    }

    #[test]
    fn test_load_history_respects_maxlen() {
        let mut buf = KlineBuffer::new(BarPeriod::OneHour, 3);
        let history = vec![
            make_bar(0, 1.0, 1.0, 1.0, 1.0),
            make_bar(3_600_000_000_000, 2.0, 2.0, 2.0, 2.0),
            make_bar(7_200_000_000_000, 3.0, 3.0, 3.0, 3.0),
            make_bar(10_800_000_000_000, 4.0, 4.0, 4.0, 4.0),
        ];
        buf.load_history(history);

        let bars = buf.bars();
        // maxlen=3，只保留最后 3 根
        assert_eq!(bars.len(), 3);
        assert_eq!(bars[0].open.as_f64(), 2.0);
        assert_eq!(bars[2].open.as_f64(), 4.0);
    }

    #[test]
    fn test_bars_includes_current_unclosed() {
        let mut buf = KlineBuffer::new(BarPeriod::OneMinute, 10);
        buf.load_history(vec![make_bar(0, 1.0, 1.0, 1.0, 1.0)]);
        buf.push_tick(&make_tick(60 * 1_000_000_000, 100.0));

        assert_eq!(buf.len(), 2); // 1 已收盘 + 1 未收盘
        let bars = buf.bars();
        assert_eq!(bars.len(), 2);
    }
}
