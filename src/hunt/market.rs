use std::fmt;

use serde::{Deserialize, Serialize};

use crate::format::rub;
use crate::hunt::category::Category;
use crate::offer::Offer;
use crate::quantity::{Quantity, Unit, UnitPrice, most_common_unit};

/// Меньше похожих предложений — и медиана скажет больше о случайности, чем о рынке.
pub const MIN_SAMPLE: usize = 5;

/// Рыночная цена: медиана цен похожих предложений.
/// Медиана, а не среднее: одно объявление «за 1 ₽» или «за миллион» её не сдвигает.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Market {
    pub median: u64,
    pub sample: usize,
}

impl Market {
    pub fn of(prices: &[u64]) -> Option<Self> {
        if prices.len() < MIN_SAMPLE {
            return None;
        }
        let mut sorted = prices.to_vec();
        sorted.sort_unstable();
        let middle = sorted.len() / 2;
        let median = if sorted.len().is_multiple_of(2) {
            u64::midpoint(sorted[middle - 1], sorted[middle])
        } else {
            sorted[middle]
        };
        Some(Self {
            median,
            sample: sorted.len(),
        })
    }
}

/// Как сравнивать цены похожих: за предложение целиком или, для расходников, за килограмм, литр, штуку.
/// Во втором случае медиана рынка — в копейках за единицу.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Pricing {
    Whole,
    PerUnit(Unit),
}

/// Рыночная цена в том виде, в каком её сравнивали: «4 446 ₽» или «310 ₽/кг».
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarketPrice {
    Whole(u64),
    PerUnit(UnitPrice),
}

/// Для расходников: цена предложения и рынка за единицу — их показывают рядом с ценой пачки.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PerUnit {
    pub price: UnitPrice,
    pub market: UnitPrice,
}

impl Pricing {
    /// Расходники сравниваются в самой частой единице фасовки. `None` — фасовки нет ни в одном названии,
    /// а сравнивать пачки неизвестного размера нельзя.
    pub fn for_offers(category: Category, offers: &[Offer]) -> Option<Self> {
        if !category.compares_per_unit() {
            return Some(Self::Whole);
        }
        most_common_unit(offers.iter().map(|offer| Quantity::in_title(&offer.name)))
            .map(|(unit, _)| Self::PerUnit(unit))
    }

    /// Цена для сравнения с рынком. `None` — у расходника нет фасовки в нужных единицах: «1 л» с килограммами не сравнить.
    pub fn comparable_price(self, offer: &Offer) -> Option<u64> {
        match self {
            Self::Whole => Some(offer.price),
            Self::PerUnit(unit) => Some(quantity(offer, unit)?.unit_price(offer.price).kopecks),
        }
    }

    pub fn market_price(self, market: Market) -> MarketPrice {
        match self {
            Self::Whole => MarketPrice::Whole(market.median),
            Self::PerUnit(unit) => MarketPrice::PerUnit(UnitPrice {
                kopecks: market.median,
                unit,
            }),
        }
    }

    /// Рынок для этого предложения. У расходника — сколько на рынке стоит такое же количество:
    /// от него считаются скидка и выгода перепродажи, как у любого другого товара.
    pub fn market_for(self, market: Market, offer: &Offer) -> (Market, Option<PerUnit>) {
        let Self::PerUnit(unit) = self else {
            return (market, None);
        };
        let Some(quantity) = quantity(offer, unit) else {
            return (market, None);
        };
        let rate = UnitPrice {
            kopecks: market.median,
            unit,
        };
        let same_amount = Market {
            median: quantity.cost_at(rate),
            ..market
        };
        let per_unit = PerUnit {
            price: quantity.unit_price(offer.price),
            market: rate,
        };
        (same_amount, Some(per_unit))
    }
}

fn quantity(offer: &Offer, unit: Unit) -> Option<Quantity> {
    Quantity::in_title(&offer.name).filter(|quantity| quantity.unit == unit)
}

impl fmt::Display for MarketPrice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MarketPrice::Whole(rubles) => formatter.write_str(&rub(*rubles)),
            MarketPrice::PerUnit(price) => price.fmt(formatter),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_ignores_outliers() {
        assert_eq!(
            Market::of(&[1, 70_000, 72_000, 75_000, 1_000_000]),
            Some(Market {
                median: 72_000,
                sample: 5
            })
        );
    }

    #[test]
    fn even_sample_takes_middle_pair() {
        assert_eq!(Market::of(&[10, 20, 30, 40, 50, 60]).map(|m| m.median), Some(35));
    }

    #[test]
    fn too_few_offers_give_no_market() {
        assert_eq!(Market::of(&[100, 200, 300, 400]), None);
    }

    #[test]
    fn consumables_compare_in_the_most_common_unit() {
        let offers = [
            Offer::sample("Порошок 3 кг", 750),
            Offer::sample("Порошок 450 г", 180),
            Offer::sample("Гель 1 л", 300),
        ];
        let pricing = Pricing::for_offers(Category::Consumables, &offers);

        assert_eq!(pricing, Some(Pricing::PerUnit(Unit::Kilogram)));
        let prices: Vec<Option<u64>> = offers.iter().map(|o| pricing.unwrap().comparable_price(o)).collect();
        assert_eq!(prices, [Some(25_000), Some(40_000), None]);
        assert_eq!(
            Pricing::for_offers(Category::Tools, &offers),
            Some(Pricing::Whole),
            "у инструментов «18 В» и «2 АКБ» — не фасовка"
        );
        assert_eq!(
            Pricing::for_offers(Category::Consumables, &[Offer::sample("Порошок", 100)]),
            None
        );
    }

    #[test]
    fn market_for_a_pack_is_the_market_price_of_the_same_amount() {
        let market = Market {
            median: 30_000,
            sample: 12,
        };
        let pricing = Pricing::PerUnit(Unit::Kilogram);
        let (same_amount, per_unit) = pricing.market_for(market, &Offer::sample("Порошок 6 кг", 1400));

        assert_eq!(same_amount.median, 1800);
        assert_eq!(per_unit.unwrap().price.to_string(), "233\u{a0}₽/кг");
        assert_eq!(pricing.market_price(market).to_string(), "300\u{a0}₽/кг");
        assert_eq!(
            Pricing::Whole.market_for(market, &Offer::sample("Порошок 6 кг", 1400)),
            (market, None)
        );
    }
}
