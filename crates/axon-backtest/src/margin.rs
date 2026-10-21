//! 永续合约保证金核算（币安分档模型）。
//!
//! 这里只做纯计算与账本状态，不碰撮合：强平动作由回测引擎执行
//! （见 `engine.rs::risk_reduce` / `check_margin`）。

/// 币安 `/fapi/v1/leverageBracket` 的 brackets[] 一一对应。
#[derive(Debug, Clone, PartialEq)]
pub struct Bracket {
    /// 该档名义下界（含）
    pub notional_floor: f64,
    /// 该档名义上界（不含）；`None` 表示最高档无上限
    pub notional_cap: Option<f64>,
    /// 该档维持保证金率
    pub mmr: f64,
    /// 币安 maintenanceAmount：仅用于自洽校验，不参与相加
    /// （分段累加与 `名义 × mmr − cum` 等价，两者相加会双重计入）
    pub cum: f64,
    /// 该档强平手续费率（币安接口不直接给，采集侧按 taker 填充）
    pub liquidation_fee_rate: f64,
    /// 该档开放的最高杠杆，用于校验用户设定的杠杆
    pub max_leverage: f64,
}

/// 保证金模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarginMode {
    /// 逐仓：每品种独立子账户，爆仓只影响该品种
    Isolated,
    /// 全仓：所有品种共享账户权益
    Cross,
}

/// 单品种的保证金规格。
#[derive(Debug, Clone, PartialEq)]
pub struct MarginSpec {
    /// 用户设定的杠杆倍数
    pub leverage: f64,
    /// 逐仓 / 全仓
    pub mode: MarginMode,
    /// 按 `notional_floor` 升序；至少 1 档
    pub brackets: Vec<Bracket>,
}

impl MarginSpec {
    /// 维持保证金：各档区间内的名义分别乘该档 MMR 后累加。
    ///
    /// 之所以分段累加而不是 `名义 × 当前档 mmr − cum`：后者依赖档位数据里的 cum
    /// 完全正确；分段累加只需要 mmr 与边界，档位漂移时更稳，且两者在数据自洽时等价。
    pub fn maintenance_margin(&self, notional: f64) -> f64 {
        if notional <= 0.0 {
            return 0.0;
        }
        let mut total = 0.0;
        for b in &self.brackets {
            let upper = b.notional_cap.unwrap_or(f64::INFINITY);
            if notional <= b.notional_floor {
                break;
            }
            let slice = notional.min(upper) - b.notional_floor;
            if slice > 0.0 {
                total += slice * b.mmr;
            }
        }
        total
    }

    /// 当前名义落在第几档（0-based），用于强平事件的 tier_from / tier_to。
    pub fn tier_index(&self, notional: f64) -> usize {
        let mut idx = 0;
        for (i, b) in self.brackets.iter().enumerate() {
            if notional > b.notional_floor {
                idx = i;
            }
        }
        idx
    }

    /// 当前名义所在档位的强平手续费率；档位表为空时按 0 处理（上层保证至少 1 档）。
    pub fn liquidation_fee_rate(&self, notional: f64) -> f64 {
        self.brackets
            .get(self.tier_index(notional))
            .map_or(0.0, |b| b.liquidation_fee_rate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn btc_brackets() -> Vec<Bracket> {
        // 币安 BTCUSDT 前两档（真实量级，cum 用于自洽校验）
        vec![
            Bracket {
                notional_floor: 0.0,
                notional_cap: Some(50_000.0),
                mmr: 0.004,
                cum: 0.0,
                liquidation_fee_rate: 0.004,
                max_leverage: 125.0,
            },
            Bracket {
                notional_floor: 50_000.0,
                notional_cap: Some(600_000.0),
                mmr: 0.005,
                cum: 50.0,
                liquidation_fee_rate: 0.004,
                max_leverage: 100.0,
            },
        ]
    }

    #[test]
    fn maintenance_margin_sums_across_tiers() {
        let spec = MarginSpec {
            leverage: 10.0,
            mode: MarginMode::Cross,
            brackets: btc_brackets(),
        };
        // 10 万名义 = 5 万 × 0.4% + 5 万 × 0.5% = 450
        assert!((spec.maintenance_margin(100_000.0) - 450.0).abs() < 1e-9);
    }

    #[test]
    fn cum_matches_tier_formula_for_sanity_check() {
        let spec = MarginSpec {
            leverage: 10.0,
            mode: MarginMode::Cross,
            brackets: btc_brackets(),
        };
        // 币安语义：MM = 名义 × 该档 mmr − cum；与分段累加必须等价
        let notional = 100_000.0;
        let flat = notional * 0.005 - 50.0;
        assert!((spec.maintenance_margin(notional) - flat).abs() < 1e-9);
    }

    #[test]
    fn zero_or_negative_notional_has_no_maintenance_margin() {
        let spec = MarginSpec {
            leverage: 5.0,
            mode: MarginMode::Isolated,
            brackets: btc_brackets(),
        };
        assert_eq!(spec.maintenance_margin(0.0), 0.0);
        assert_eq!(spec.maintenance_margin(-1.0), 0.0);
    }
}
