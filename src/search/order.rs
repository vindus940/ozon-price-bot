use std::cmp::Reverse;

use crate::offer::Offer;
use crate::search::Finding;

/// Оценка, о которой ничего не известно: ниже типичных 4.7–4.9 на Ozon, чтобы новинки без отзывов не лезли вверх.
const PRIOR_RATING: f64 = 4.0;
/// Сколько отзывов нужно, чтобы оценка товара весила столько же, сколько осторожная 4.0.
const PRIOR_REVIEWS: f64 = 20.0;

/// В каком порядке показывать подходящие: на разные вопросы — разные ответы.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    #[default]
    Cheapest,
    /// Самая большая скидка от старой цены. Без скидки — после всех со скидкой.
    BiggestDiscount,
    /// И недорого, и хорошо оценено многими: место по цене плюс место по надёжности оценки.
    PriceAndRating,
}

impl Order {
    /// Если просили цену за единицу, предложения без фасовки в этих единицах в любом порядке идут после остальных:
    /// их цену не с чем сравнить.
    pub fn arrange(self, mut findings: Vec<Finding>) -> Vec<Finding> {
        match self {
            Order::Cheapest => findings.sort_by_key(|found| (found.unit_price.is_none(), cost(found))),
            Order::BiggestDiscount => findings.sort_by_key(|found| {
                (
                    found.unit_price.is_none(),
                    Reverse(found.offer.discount_percent().unwrap_or(0)),
                    cost(found),
                )
            }),
            Order::PriceAndRating => {
                let scores: Vec<usize> = findings.iter().map(|found| place(found, &findings)).collect();
                let mut ranked: Vec<(usize, Finding)> = scores.into_iter().zip(findings).collect();
                ranked.sort_by_key(|(score, found)| (found.unit_price.is_none(), *score, cost(found)));
                findings = ranked.into_iter().map(|(_, found)| found).collect();
            }
        }
        findings
    }
}

/// Цена для сравнения внутри своей группы: за единицу у тех, у кого она есть, иначе за предложение.
fn cost(found: &Finding) -> u64 {
    found.unit_price.map_or(found.offer.price, |price| price.kopecks)
}

/// Сколько предложений дешевле этого плюс сколько оценены надёжнее. Меньше — лучше.
/// Места, а не сами цены и оценки: так рубли и звёзды не нужно переводить друг в друга.
fn place(found: &Finding, all: &[Finding]) -> usize {
    let peers: Vec<&Finding> = all
        .iter()
        .filter(|other| other.unit_price.is_some() == found.unit_price.is_some())
        .collect();
    let cheaper = peers.iter().filter(|other| cost(other) < cost(found)).count();
    let rating = trusted_rating(&found.offer);
    let better_rated = peers
        .iter()
        .filter(|other| trusted_rating(&other.offer) > rating)
        .count();
    cheaper + better_rated
}

/// Оценка с поправкой на число отзывов: «5.0 по двум отзывам» тянется к осторожным 4.0,
/// а «4.8 по восьми тысячам» почти не меняется.
fn trusted_rating(offer: &Offer) -> f64 {
    let rating = offer.rating.map_or(PRIOR_RATING, f64::from);
    let reviews = offer
        .reviews
        .map_or(0.0, |reviews| f64::from(u32::try_from(reviews).unwrap_or(u32::MAX)));
    (rating * reviews + PRIOR_RATING * PRIOR_REVIEWS) / (reviews + PRIOR_REVIEWS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::quantity::{Unit, UnitPrice};

    fn found(name: &str, price: u64) -> Finding {
        Finding {
            offer: Offer::sample(name, price),
            unit_price: None,
        }
    }

    fn names(findings: &[Finding]) -> Vec<&str> {
        findings.iter().map(|found| found.offer.name.as_str()).collect()
    }

    fn rated(name: &str, price: u64, rating: f32, reviews: u64) -> Finding {
        Finding {
            offer: Offer {
                rating: Some(rating),
                reviews: Some(reviews),
                ..Offer::sample(name, price)
            },
            unit_price: None,
        }
    }

    fn discounted(name: &str, price: u64, old_price: Option<u64>) -> Finding {
        Finding {
            offer: Offer {
                old_price,
                ..Offer::sample(name, price)
            },
            unit_price: None,
        }
    }

    #[test]
    fn cheapest_first_and_unmeasured_after_measured() {
        let per_kilogram = |name: &str, price: u64, kopecks: u64| Finding {
            unit_price: Some(UnitPrice {
                kopecks,
                unit: Unit::Kilogram,
            }),
            ..found(name, price)
        };
        let arranged = Order::Cheapest.arrange(vec![
            found("без веса", 90),
            per_kilogram("450 г", 180, 40_000),
            per_kilogram("6 кг", 1_400, 23_333),
        ]);
        assert_eq!(names(&arranged), ["6 кг", "450 г", "без веса"]);
    }

    #[test]
    fn biggest_discount_first_then_no_discount_by_price() {
        let arranged = Order::BiggestDiscount.arrange(vec![
            discounted("без скидки дешёвый", 900, None),
            discounted("−20%", 4_000, Some(5_000)),
            discounted("−50% дорогой", 6_000, Some(12_000)),
            discounted("−50% дешёвый", 2_000, Some(4_000)),
            discounted("без скидки", 1_500, None),
        ]);
        assert_eq!(
            names(&arranged),
            [
                "−50% дешёвый",
                "−50% дорогой",
                "−20%",
                "без скидки дешёвый",
                "без скидки"
            ]
        );
    }

    #[test]
    fn price_and_rating_prefers_well_rated_by_many_over_cheap_or_unproven() {
        let arranged = Order::PriceAndRating.arrange(vec![
            rated("дешёвые с плохой оценкой", 300, 4.1, 40),
            rated("новинка с двумя отзывами", 900, 5.0, 2),
            rated("хит: 4.8 по 8000 отзывов", 1_500, 4.8, 8_000),
            rated("неплохие", 1_100, 4.6, 2_500),
            rated("средние", 2_500, 4.7, 5_000),
            rated("премиум", 6_000, 4.9, 12_000),
        ]);
        assert_eq!(arranged[0].offer.name, "хит: 4.8 по 8000 отзывов");
    }

    #[test]
    fn few_reviews_pull_rating_down_to_a_cautious_four() {
        let rating = |stars: f32, reviews: u64| {
            trusted_rating(&Offer {
                rating: Some(stars),
                reviews: Some(reviews),
                ..Offer::sample("товар", 100)
            })
        };
        assert!(rating(5.0, 2) < 4.2);
        assert!(rating(4.8, 8_000) > 4.79);
        assert!((trusted_rating(&Offer::sample("без оценки", 100)) - PRIOR_RATING).abs() < 1e-9);
    }
}
