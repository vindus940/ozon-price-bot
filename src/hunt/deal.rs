use chrono::{DateTime, Local};
use serde::{Deserialize, Serialize};

use crate::hunt::market::{Market, PerUnit};
use crate::hunt::watch::Goal;
use crate::offer::Offer;

/// Дешевле половины рынка — у частников это чаще мошенник или скрытый дефект, чем удача.
const SUSPICIOUS_DISCOUNT_PERCENT: u64 = 50;
/// При перепродаже покупатель почти всегда торгуется.
const HAGGLE_PERCENT: u64 = 10;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Deal {
    pub watch_id: u64,
    pub watch_name: String,
    pub found_at: DateTime<Local>,
    pub offer: Offer,
    /// Сколько на рынке стоит такое же предложение; у расходников — такое же количество товара.
    pub market_price: u64,
    pub market_sample: usize,
    pub discount_percent: u64,
    pub resale_profit: Option<i64>,
    pub suspicious: bool,
    #[serde(default)]
    pub per_unit: Option<PerUnit>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Verdict {
    pub discount_percent: u64,
    pub resale_profit: Option<i64>,
    pub suspicious: bool,
}

/// Неизвестное расстояние радиус не проходит: «до 5 км» значит «точно не дальше 5 км».
pub fn within_radius(radius_km: Option<u32>, offer: &Offer) -> bool {
    radius_km.is_none_or(|radius| offer.distance_km.is_some_and(|km| km <= f64::from(radius)))
}

/// Выгодно ли предложение для этой цели. `None` — не выгодно или дальше радиуса.
pub fn evaluate(goal: Goal, radius_km: Option<u32>, market: Market, offer: &Offer) -> Option<Verdict> {
    if !within_radius(radius_km, offer) {
        return None;
    }

    let discount_percent = market.median.saturating_sub(offer.price) * 100 / market.median.max(1);
    let suspicious = discount_percent > SUSPICIOUS_DISCOUNT_PERCENT;

    match goal {
        Goal::BelowMarket { min_discount_percent } => {
            (discount_percent >= u64::from(min_discount_percent)).then_some(Verdict {
                discount_percent,
                resale_profit: None,
                suspicious,
            })
        }
        Goal::Resale { min_profit, costs } => {
            let expected_sale = market.median * (100 - HAGGLE_PERCENT) / 100;
            let profit = i64::try_from(expected_sale).ok()? - i64::try_from(offer.price + costs).ok()?;
            (profit >= i64::try_from(min_profit).ok()?).then_some(Verdict {
                discount_percent,
                resale_profit: Some(profit),
                suspicious,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MARKET: Market = Market {
        median: 70_000,
        sample: 12,
    };

    fn priced(price: u64, distance_km: Option<f64>) -> Offer {
        Offer {
            distance_km,
            ..Offer::sample("iPhone 15 128GB", price)
        }
    }

    #[test]
    fn below_market_needs_enough_discount() {
        let goal = Goal::BelowMarket {
            min_discount_percent: 20,
        };
        assert_eq!(
            evaluate(goal, None, MARKET, &priced(56_000, None)).map(|v| v.discount_percent),
            Some(20)
        );
        assert_eq!(evaluate(goal, None, MARKET, &priced(57_000, None)), None);
        assert_eq!(evaluate(goal, None, MARKET, &priced(90_000, None)), None);
    }

    #[test]
    fn half_price_is_flagged_as_suspicious() {
        let goal = Goal::BelowMarket {
            min_discount_percent: 20,
        };
        assert!(evaluate(goal, None, MARKET, &priced(30_000, None)).unwrap().suspicious);
        assert!(!evaluate(goal, None, MARKET, &priced(50_000, None)).unwrap().suspicious);
    }

    #[test]
    fn radius_keeps_only_offers_known_to_be_close() {
        let goal = Goal::BelowMarket {
            min_discount_percent: 20,
        };
        assert!(evaluate(goal, Some(5), MARKET, &priced(50_000, Some(4.9))).is_some());
        assert!(evaluate(goal, Some(5), MARKET, &priced(50_000, Some(5.1))).is_none());
        assert!(evaluate(goal, Some(5), MARKET, &priced(50_000, None)).is_none());
    }

    #[test]
    fn resale_counts_haggle_and_costs() {
        let goal = Goal::Resale {
            min_profit: 5_000,
            costs: 1_000,
        };
        let verdict = evaluate(goal, Some(5), MARKET, &priced(55_000, Some(2.4))).unwrap();
        assert_eq!(verdict.resale_profit, Some(70_000 * 90 / 100 - 55_000 - 1_000));
        assert_eq!(evaluate(goal, Some(5), MARKET, &priced(58_500, Some(2.4))), None);
    }
}
