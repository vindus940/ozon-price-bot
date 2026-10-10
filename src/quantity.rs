use std::fmt;
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::format::rub;
use crate::text::normalize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    Kilogram,
    Liter,
    Piece,
}

impl Unit {
    pub const ALL: [Unit; 3] = [Unit::Kilogram, Unit::Liter, Unit::Piece];

    pub fn label(self) -> &'static str {
        match self {
            Unit::Kilogram => "кг",
            Unit::Liter => "л",
            Unit::Piece => "шт",
        }
    }
}

/// Сколько товара в предложении — в килограммах, литрах или штуках.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quantity {
    pub amount: f64,
    pub unit: Unit,
}

/// Цена за килограмм, литр или штуку. В копейках: салфетка или ватная палочка стоят меньше рубля.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnitPrice {
    pub kopecks: u64,
    pub unit: Unit,
}

const AMOUNT: &str = r"(?P<amount>\d+(?:[.,]\d+)?)";
/// Длинные формы раньше коротких: «грамм» раньше «г», «литр» раньше «л». Латинские «g» и «l» не берутся:
/// «5G» в названии смартфона — это связь, а не граммы.
const MEASURE: &str = r"(?P<measure>кг|kg|грамм\w*|гр|г|мл|ml|литр\w*|л)";
const COUNT: &str = r"(?P<count>\d+)";
const TIMES: &str = r"\s*(?:x|х|×|\*)\s*";
const PACKS: &str = r"(?:шт|штук|уп|упаковк|пачк|бутыл|банк)\w*\.?";

/// Упаковки из нескольких одинаковых: «6 x 1,5 л», «2шт х 400 г», «500 г х 2», «3 упаковки по 500 г», «1,5 л, 6 шт».
static MULTIPACKS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    [
        format!(r"{COUNT}\s*(?:{PACKS})?{TIMES}{AMOUNT}\s*{MEASURE}\b"),
        format!(r"{AMOUNT}\s*{MEASURE}{TIMES}{COUNT}\b"),
        format!(r"{COUNT}\s*{PACKS}\s*по\s*{AMOUNT}\s*{MEASURE}\b"),
        format!(r"{AMOUNT}\s*{MEASURE}[\s,]+{COUNT}\s*{PACKS}"),
    ]
    .iter()
    .map(|pattern| Regex::new(pattern).unwrap())
    .collect()
});
/// «3 кг», «450г», «1,5 л». Граница слова не даёт принять «128 гб» за граммы.
static SINGLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(&format!(r"{AMOUNT}\s*{MEASURE}\b")).unwrap());
/// «60 шт», «12 рулонов», «30 капсул», «100 листов».
static PIECES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(\d+)\s*(?:шт|штук|капсул|таблет|пакетик|рулон|лист|салфет)\w*\.?(?:\W|$)").unwrap());

impl Quantity {
    /// Фасовка из названия. Вес и объём важнее штук: в «Порошок 3 кг, 30 стирок» считаются килограммы.
    pub fn in_title(title: &str) -> Option<Self> {
        let text = normalize(title);
        let multipack = MULTIPACKS.iter().find_map(|pattern| {
            let found = pattern.captures(&text)?;
            Some(Self::measured(&found["amount"], &found["measure"])?.times(&found["count"]))
        });
        multipack
            .or_else(|| {
                SINGLE
                    .captures_iter(&text)
                    .find_map(|found| Self::measured(&found["amount"], &found["measure"]))
            })
            .or_else(|| {
                let pieces: f64 = PIECES.captures(&text)?[1].parse().ok()?;
                (pieces > 0.0).then_some(Self {
                    amount: pieces,
                    unit: Unit::Piece,
                })
            })
    }

    pub fn unit_price(self, price: u64) -> UnitPrice {
        UnitPrice {
            kopecks: scaled(price * 100, 1.0 / self.amount),
            unit: self.unit,
        }
    }

    /// Сколько рублей стоит такое количество по цене за единицу: рынок для пачки этого размера.
    pub fn cost_at(self, rate: UnitPrice) -> u64 {
        scaled(rate.kopecks, self.amount / 100.0)
    }

    fn measured(amount: &str, measure: &str) -> Option<Self> {
        let fractional = amount.contains([',', '.']);
        let value: f64 = amount
            .replace(',', ".")
            .parse()
            .ok()
            .filter(|&value: &f64| value > 0.0)?;
        // «Мёд 2024 г. урожая» — это год, а не два килограмма.
        if measure == "г" && !fractional && (1900.0..2100.0).contains(&value) {
            return None;
        }
        let (amount, unit) = match measure {
            "кг" | "kg" => (value, Unit::Kilogram),
            "мл" | "ml" => (value / 1000.0, Unit::Liter),
            liters if liters.starts_with('л') => (value, Unit::Liter),
            _ => (value / 1000.0, Unit::Kilogram),
        };
        Some(Self { amount, unit })
    }

    fn times(self, count: &str) -> Self {
        let count: f64 = count.parse().unwrap_or(1.0);
        Self {
            amount: self.amount * count.max(1.0),
            ..self
        }
    }
}

/// Цены и копейки здесь намного меньше 2⁵³, а результат положителен — переводы между f64 и u64 точны.
#[allow(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss
)]
fn scaled(value: u64, factor: f64) -> u64 {
    (value as f64 * factor).round() as u64
}

/// Самая частая единица фасовки и сколько раз она встретилась. При равенстве — в порядке кг, л, шт.
pub fn most_common_unit(quantities: impl IntoIterator<Item = Option<Quantity>>) -> Option<(Unit, usize)> {
    let units: Vec<Unit> = quantities.into_iter().flatten().map(|quantity| quantity.unit).collect();
    Unit::ALL
        .into_iter()
        .map(|unit| (unit, units.iter().filter(|&&found| found == unit).count()))
        .filter(|&(_, count)| count > 0)
        .reduce(|best, next| if next.1 > best.1 { next } else { best })
}

/// «250 ₽/кг», а меньше сотни рублей — с копейками: «0,89 ₽/шт».
impl fmt::Display for UnitPrice {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (rubles, kopecks) = (self.kopecks / 100, self.kopecks % 100);
        if rubles >= 100 || kopecks == 0 {
            write!(formatter, "{}/{}", rub((self.kopecks + 50) / 100), self.unit.label())
        } else {
            write!(formatter, "{rubles},{kopecks:02}\u{a0}₽/{}", self.unit.label())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(title: &str) -> Option<(f64, Unit)> {
        Quantity::in_title(title).map(|q| ((q.amount * 1000.0).round() / 1000.0, q.unit))
    }

    #[test]
    fn weight_and_volume_in_any_spelling() {
        assert_eq!(parsed("Стиральный порошок Persil 3 кг"), Some((3.0, Unit::Kilogram)));
        assert_eq!(parsed("Порошок Ariel 450г"), Some((0.45, Unit::Kilogram)));
        assert_eq!(parsed("Кофе молотый 250 гр."), Some((0.25, Unit::Kilogram)));
        assert_eq!(parsed("Гель для стирки 1,5 л"), Some((1.5, Unit::Liter)));
        assert_eq!(parsed("Шампунь 250 мл"), Some((0.25, Unit::Liter)));
        assert_eq!(parsed("Масло моторное 4 литра"), Some((4.0, Unit::Liter)));
        assert_eq!(parsed("Корм Royal Canin 2kg"), Some((2.0, Unit::Kilogram)));
    }

    #[test]
    fn multipacks_are_multiplied() {
        assert_eq!(parsed("Вода минеральная 6 x 1,5 л"), Some((9.0, Unit::Liter)));
        assert_eq!(parsed("Йогурт 2×100 мл"), Some((0.2, Unit::Liter)));
        assert_eq!(parsed("Кофе в зёрнах 500 г х 2"), Some((1.0, Unit::Kilogram)));
        assert_eq!(parsed("Корм для кошек 3 шт по 2 кг"), Some((6.0, Unit::Kilogram)));
        assert_eq!(parsed("Масло 3 упаковки по 500 г"), Some((1.5, Unit::Kilogram)));
        assert_eq!(parsed("Вода питьевая 1,5 л, 6 шт"), Some((9.0, Unit::Liter)));
    }

    #[test]
    fn pieces_when_there_is_no_weight_or_volume() {
        assert_eq!(parsed("Капсулы для стирки 60 шт"), Some((60.0, Unit::Piece)));
        assert_eq!(parsed("Туалетная бумага 12 рулонов"), Some((12.0, Unit::Piece)));
        assert_eq!(parsed("Салфетки бумажные 100 листов"), Some((100.0, Unit::Piece)));
        assert_eq!(parsed("Порошок 3 кг, 30 стирок"), Some((3.0, Unit::Kilogram)));
    }

    #[test]
    fn numbers_that_are_not_a_pack_size() {
        assert_eq!(parsed("Смартфон Apple iPhone 15 128 ГБ"), None);
        assert_eq!(parsed("Смартфон Samsung Galaxy A55 5G 8/256"), None);
        assert_eq!(parsed("Шуруповерт Makita 18 В, 2 АКБ по 2 Ач"), None);
        assert_eq!(parsed("Пауэрбанк 10000 мАч"), None);
        assert_eq!(parsed("Ланч-бокс стеклянный"), None);
        assert_eq!(parsed("Мёд липовый 2024 г. урожая, 1 кг"), Some((1.0, Unit::Kilogram)));
    }

    #[test]
    fn unit_price_and_back() {
        let pack = Quantity::in_title("Порошок 450 г").unwrap();
        let rate = pack.unit_price(180);
        assert_eq!(
            rate,
            UnitPrice {
                kopecks: 40_000,
                unit: Unit::Kilogram
            }
        );
        assert_eq!(Quantity::in_title("Порошок 3 кг").unwrap().cost_at(rate), 1_200);
    }

    #[test]
    fn unit_price_reads_like_a_price_tag() {
        let price = |kopecks| UnitPrice {
            kopecks,
            unit: Unit::Kilogram,
        };
        assert_eq!(price(40_000).to_string(), "400\u{a0}₽/кг");
        assert_eq!(price(123_456).to_string(), "1\u{a0}235\u{a0}₽/кг");
        assert_eq!(price(2_500).to_string(), "25\u{a0}₽/кг");
        assert_eq!(
            UnitPrice {
                kopecks: 89,
                unit: Unit::Piece
            }
            .to_string(),
            "0,89\u{a0}₽/шт"
        );
    }

    #[test]
    fn most_common_unit_counts_only_known_pack_sizes() {
        let titles = ["Порошок 3 кг", "Порошок 450 г", "Гель 1 л", "Порошок без веса"];
        assert_eq!(
            most_common_unit(titles.map(Quantity::in_title)),
            Some((Unit::Kilogram, 2))
        );
        assert_eq!(most_common_unit(["Наушники Sony"].map(Quantity::in_title)), None);
    }
}
