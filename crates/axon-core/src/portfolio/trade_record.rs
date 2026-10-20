//! 交易记录

use serde::{Deserialize, Serialize};

use crate::market::Trade;
use crate::types::Instrument;

/// 交易记录（用于审计和盈亏计算）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TradeRecord {
    /// 成交记录
    pub trade: Trade,
    /// 已实现盈亏（单位：f64 × 1e6）
    pub realized_pnl: i64,
    /// 佣金（单位：f64 × 1e6）
    pub commission: i64,
    /// 净数量（带方向符号）
    pub net_quantity: i64,
    /// T2.4 新增:成交所属的 Instrument(spot/swap)
    ///
    /// 为何单独持有而非从 `Trade` 派生:
    /// - `Trade` 固定 40 字节(`#[repr(C)]`),hot path 必须保持紧凑
    /// - `TradeRecord` 走 `trades: Vec<TradeRecord>` 审计路径,每根 bar 最多
    ///   几次 push,允许额外字段
    /// - 后期 report / PnL 拆分(per-instrument 累计)直接用此字段
    #[serde(default)]
    pub instrument: Instrument,
    /// T0.17 新增:开仓均价(单位 f64 × 1e6),取 PositionState.avg_cost
    ///
    /// 与引擎 realized_pnl 口径一致(均用 avg_cost 而非逐笔 FIFO 开仓价),
    /// 保证 Σ trade.realized_pnl == result.total_pnl。
    #[serde(default)]
    pub entry_price: i64,
    /// T0.17 新增:首次开仓时间(纳秒),取 PositionState.entry_time_ns
    #[serde(default)]
    pub entry_time_ns: i64,
    /// 构成该持仓的开仓 fill id 列表(同向加仓会追加多条)
    #[serde(default)]
    pub entry_fill_ids: Vec<u32>,
    /// 触发本次平仓的 fill id(0 表示未知/兼容实盘路径)
    #[serde(default)]
    pub exit_fill_id: u32,
}

impl TradeRecord {
    /// 创建新交易记录
    ///
    /// # 参数
    ///
    /// - `trade`:成交记录(40 字节紧凑表示)
    /// - `realized_pnl`:已实现盈亏(单位 f64 × 1e6)
    /// - `commission`:佣金(单位 f64 × 1e6)
    /// - `net_quantity`:净数量(带方向符号,f64 × 1e6)
    /// - `instrument`:成交所属的 Instrument
    /// - `entry_price`:开仓均价(单位 f64 × 1e6,PositionState.avg_cost)
    /// - `entry_time_ns`:首次开仓时间(纳秒,PositionState.entry_time_ns)
    /// - `entry_fill_ids`:构成该持仓的开仓 fill id 列表(同向加仓会追加多条)
    /// - `exit_fill_id`:触发本次平仓的 fill id(0 表示未知/兼容实盘路径)
    // 构造器字段一对一透传,拆成 builder 反而增加调用点噪声
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        trade: Trade,
        realized_pnl: i64,
        commission: i64,
        net_quantity: i64,
        instrument: Instrument,
        entry_price: i64,
        entry_time_ns: i64,
        entry_fill_ids: Vec<u32>,
        exit_fill_id: u32,
    ) -> Self {
        Self {
            trade,
            realized_pnl,
            commission,
            net_quantity,
            instrument,
            entry_price,
            entry_time_ns,
            entry_fill_ids,
            exit_fill_id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time::Timestamp;
    use crate::types::{Price, Quantity, SpotInstrument, Symbol};

    fn btc_usdt() -> Instrument {
        Instrument::Spot(SpotInstrument {
            base: Symbol::from("BTC"),
            quote: Symbol::from("USDT"),
        })
    }

    #[test]
    fn test_trade_record_creation() {
        let trade = Trade::new(
            Timestamp::from_nanos(1_000),
            Price::from_f64(100.0),
            Quantity::from_f64(1.0),
            1,
            2,
        );
        let rec = TradeRecord::new(
            trade,
            1_000_000,
            100_000,
            1_000_000,
            btc_usdt(),
            10_000_000,
            1_000,
            Vec::new(),
            0,
        );
        assert_eq!(rec.realized_pnl, 1_000_000);
        assert_eq!(rec.commission, 100_000);
        assert_eq!(rec.instrument, btc_usdt());
        assert_eq!(rec.entry_price, 10_000_000);
        assert_eq!(rec.entry_time_ns, 1_000);
    }

    #[test]
    fn test_trade_record_fill_association() {
        let trade = Trade::new(
            Timestamp::from_nanos(1_000),
            Price::from_f64(100.0),
            Quantity::from_f64(1.0),
            1,
            2,
        );
        let rec = TradeRecord::new(
            trade,
            1_000_000,
            100_000,
            1_000_000,
            btc_usdt(),
            10_000_000,
            1_000,
            vec![0, 1],
            2,
        );
        assert_eq!(rec.entry_fill_ids, vec![0, 1]);
        assert_eq!(rec.exit_fill_id, 2);
    }
}
