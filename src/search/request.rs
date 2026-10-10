use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::offer::Offer;
use crate::quantity::Unit;
use crate::search::Order;

#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PriceRange {
    pub min: Option<u64>,
    pub max: Option<u64>,
}

impl PriceRange {
    pub fn contains(&self, price: u64) -> bool {
        self.min.is_none_or(|min| price >= min) && self.max.is_none_or(|max| price <= max)
    }
}

/// Сообщение пользователя, разобранное на части:
/// «наушники sony -амбушюры до 5000 рейтинг от 4.5 отзывов от 100», «стиральный порошок за кг», «чайник по скидке».
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SearchRequest {
    pub query: String,
    pub excluded: Vec<String>,
    pub price: PriceRange,
    pub min_rating: Option<f32>,
    pub min_reviews: Option<u64>,
    /// Сравнивать по цене за килограмм, литр или штуку. Только по просьбе: у чайника «1,7 л» в названии —
    /// это объём, но цена за литр чайника ничего не значит.
    pub per_unit: Option<Unit>,
    pub order: Order,
    pub ozon: OzonConditions,
}

/// Условия, которые понимает только Ozon: у объявлений Авито нет ни срока доставки, ни продавца-маркетплейса.
#[derive(Debug, Default, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct OzonConditions {
    /// «доставка завтра»: привезут не позже завтра.
    pub by_tomorrow: bool,
    /// «только Ozon»: продаёт сам маркетплейс, а не сторонний продавец.
    pub only_ozon: bool,
}

impl OzonConditions {
    pub fn accepts(self, offer: &Offer) -> bool {
        (!self.by_tomorrow || offer.arrives_by_tomorrow()) && (!self.only_ozon || offer.sold_by_ozon)
    }

    /// Как условия звучат в заголовке ответа и в строке слежки.
    pub fn labels(self) -> Vec<&'static str> {
        [
            self.by_tomorrow.then_some("доставка до завтра"),
            self.only_ozon.then_some("только Ozon"),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// «доставка завтра», «с доставкой сегодня», «доставка до завтра» — всё значит «не позже завтра».
static BY_TOMORROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)(?:с\s+)?(доставк\w*\s+(?:до\s+)?(?:завтра|сегодня))(?:\s|$)").unwrap());

static ONLY_OZON: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)(только\s+(?:ozon|озон\w*))(?:\s|$)").unwrap());

static PER_UNIT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)за\s+(?:1\s*)?(кг|килограмм\w*|л|литр\w*|шт|штук\w*)\b").unwrap());

/// «по скидке», «цена-качество», «цена/качество», «цена качество».
static ORDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)(по\s+скидк\w*|цена\s*[-–—/]?\s*качеств\w*)(?:\s|$)").unwrap());

static RATING_BOUND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)рейтинг\w*\s+от\s*(\d(?:[.,]\d{1,2})?)").unwrap());

static REVIEWS_BOUND: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|\s)отзыв\w*\s+от\s*(\d{1,3}(?:[ \u{a0}]\d{3})+|\d+)").unwrap());

static PRICE_BOUND: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|\s)(от|до)\s*(\d{1,3}(?:[ \u{a0}]\d{3})+|\d+)(?:\s*(?:₽|руб\b\.?|р\b\.?))?").unwrap()
});

impl SearchRequest {
    pub fn parse(text: &str) -> Self {
        let (per_unit, text) = cut(&PER_UNIT, text, |word| Some(unit(word)));
        // Рейтинг и отзывы вырезаются раньше цены: иначе «отзывов от 100» прочиталось бы как «цена от 100 ₽».
        let (min_rating, text) = cut(&RATING_BOUND, &text, |value| value.replace(',', ".").parse().ok());
        let (min_reviews, text) = cut(&REVIEWS_BOUND, &text, whole_number);
        let (order, text) = cut(&ORDER, &text, |words| Some(order(words)));
        let (by_tomorrow, text) = cut(&BY_TOMORROW, &text, |_| Some(()));
        let (only_ozon, text) = cut(&ONLY_OZON, &text, |_| Some(()));
        let (price, text) = cut_price(&text);

        let (excluded, included): (Vec<&str>, Vec<&str>) = text
            .split_whitespace()
            .partition(|word| word.len() > 1 && word.starts_with('-'));

        Self {
            query: included.join(" "),
            excluded: excluded.iter().map(|word| word[1..].to_string()).collect(),
            price,
            min_rating,
            min_reviews,
            per_unit,
            order: order.unwrap_or_default(),
            ozon: OzonConditions {
                by_tomorrow: by_tomorrow.is_some(),
                only_ozon: only_ozon.is_some(),
            },
        }
    }
}

fn order(words: &str) -> Order {
    if words.to_lowercase().contains("скидк") {
        Order::BiggestDiscount
    } else {
        Order::PriceAndRating
    }
}

fn unit(word: &str) -> Unit {
    match word.to_lowercase().chars().next() {
        Some('к') => Unit::Kilogram,
        Some('л') => Unit::Liter,
        _ => Unit::Piece,
    }
}

fn cut<T>(condition: &Regex, text: &str, value: impl Fn(&str) -> Option<T>) -> (Option<T>, String) {
    let found = condition.captures_iter(text).last().and_then(|found| value(&found[1]));
    (found, condition.replace_all(text, " ").into_owned())
}

fn cut_price(text: &str) -> (PriceRange, String) {
    let mut price = PriceRange::default();
    for bound in PRICE_BOUND.captures_iter(text) {
        let amount = whole_number(&bound[2]);
        if bound[1].to_lowercase() == "от" {
            price.min = amount;
        } else {
            price.max = amount;
        }
    }
    (price, PRICE_BOUND.replace_all(text, " ").into_owned())
}

fn whole_number(text: &str) -> Option<u64> {
    text.chars()
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(query: &str, excluded: &[&str], min: Option<u64>, max: Option<u64>) -> SearchRequest {
        SearchRequest {
            query: query.into(),
            excluded: excluded.iter().map(ToString::to_string).collect(),
            price: PriceRange { min, max },
            ..SearchRequest::default()
        }
    }

    #[test]
    fn price_bounds_are_cut_from_query() {
        assert_eq!(
            SearchRequest::parse("наушники sony до 5000"),
            request("наушники sony", &[], None, Some(5000))
        );
        assert_eq!(
            SearchRequest::parse("iphone 15 от 50 000 до 80 000"),
            request("iphone 15", &[], Some(50000), Some(80000))
        );
        assert_eq!(
            SearchRequest::parse("до 3000р кроссовки"),
            request("кроссовки", &[], None, Some(3000))
        );
        assert_eq!(
            SearchRequest::parse("От 1000 ₽ рюкзак"),
            request("рюкзак", &[], Some(1000), None)
        );
    }

    #[test]
    fn words_that_start_like_currency_stay_in_query() {
        assert_eq!(
            SearchRequest::parse("до 2000 рубашка"),
            request("рубашка", &[], None, Some(2000))
        );
        assert_eq!(
            SearchRequest::parse("подушка до 900 рюкзак"),
            request("подушка рюкзак", &[], None, Some(900))
        );
    }

    #[test]
    fn minus_words_are_separated() {
        assert_eq!(
            SearchRequest::parse("наушники sony wh-1000xm5 -амбушюры -чехол до 30000"),
            request("наушники sony wh-1000xm5", &["амбушюры", "чехол"], None, Some(30000))
        );
    }

    #[test]
    fn rating_and_reviews_bounds_are_cut_from_query() {
        assert_eq!(
            SearchRequest::parse("наушники sony рейтинг от 4.5 отзывов от 100"),
            SearchRequest {
                min_rating: Some(4.5),
                min_reviews: Some(100),
                ..request("наушники sony", &[], None, None)
            }
        );
        assert_eq!(
            SearchRequest::parse("Рейтингом от 4,8 отзывы от 1 000 чайник"),
            SearchRequest {
                min_rating: Some(4.8),
                min_reviews: Some(1000),
                ..request("чайник", &[], None, None)
            }
        );
    }

    #[test]
    fn reviews_bound_is_not_mistaken_for_price() {
        assert_eq!(
            SearchRequest::parse("чайник -стекло до 3000 отзывов от 100 рейтинг от 4"),
            SearchRequest {
                min_rating: Some(4.0),
                min_reviews: Some(100),
                ..request("чайник", &["стекло"], None, Some(3000))
            }
        );
    }

    #[test]
    fn unit_price_is_asked_in_words() {
        assert_eq!(
            SearchRequest::parse("стиральный порошок за кг рейтинг от 4.5"),
            SearchRequest {
                min_rating: Some(4.5),
                per_unit: Some(Unit::Kilogram),
                ..request("стиральный порошок", &[], None, None)
            }
        );
        assert_eq!(
            SearchRequest::parse("гель для стирки за литр").per_unit,
            Some(Unit::Liter)
        );
        assert_eq!(
            SearchRequest::parse("За 1 шт капсулы до 2000").per_unit,
            Some(Unit::Piece)
        );
        assert_eq!(SearchRequest::parse("игрушка подвес за лапку").per_unit, None);
    }

    #[test]
    fn delivery_and_seller_conditions_are_cut_from_query() {
        assert_eq!(
            SearchRequest::parse("наушники sony доставка завтра только Ozon до 5000"),
            SearchRequest {
                ozon: OzonConditions {
                    by_tomorrow: true,
                    only_ozon: true,
                },
                ..request("наушники sony", &[], None, Some(5000))
            }
        );
        for text in [
            "чайник с доставкой сегодня",
            "чайник доставка до завтра",
            "чайник только озон",
        ] {
            assert_eq!(SearchRequest::parse(text).query, "чайник", "{text}");
        }
        let plain = SearchRequest::parse("доставка воды");
        assert_eq!(
            (plain.query.as_str(), plain.ozon),
            ("доставка воды", OzonConditions::default())
        );
    }

    #[test]
    fn order_is_asked_in_words() {
        assert_eq!(
            SearchRequest::parse("наушники sony по скидке до 5000"),
            SearchRequest {
                order: Order::BiggestDiscount,
                ..request("наушники sony", &[], None, Some(5000))
            }
        );
        for text in [
            "наушники цена-качество",
            "наушники цена/качество",
            "Цена качество наушники",
        ] {
            let request = SearchRequest::parse(text);
            assert_eq!(
                (request.query.as_str(), request.order),
                ("наушники", Order::PriceAndRating),
                "{text}"
            );
        }
        assert_eq!(SearchRequest::parse("наушники").order, Order::Cheapest);
        assert_eq!(
            SearchRequest::parse("карта по скидочной программе").order,
            Order::Cheapest
        );
    }

    #[test]
    fn price_range_bounds_are_inclusive() {
        let range = PriceRange {
            min: Some(100),
            max: Some(200),
        };
        assert!(range.contains(100) && range.contains(200));
        assert!(!range.contains(99) && !range.contains(201));
        assert!(PriceRange::default().contains(u64::MAX));
    }
}
