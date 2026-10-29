use crate::offer::Offer;
use crate::search::synonyms::spellings;
use crate::search::{PriceRange, SearchRequest, ShopConditions};
use crate::text::words;

pub struct OfferFilter {
    required: Vec<Keyword>,
    excluded: Vec<Keyword>,
    price: PriceRange,
    min_rating: Option<f32>,
    min_reviews: Option<u64>,
    shop: ShopConditions,
}

impl OfferFilter {
    pub fn new(request: &SearchRequest) -> Self {
        Self {
            required: Keyword::all_in(&request.query),
            excluded: request.excluded.iter().flat_map(|word| Keyword::all_in(word)).collect(),
            price: request.price,
            min_rating: request.min_rating,
            min_reviews: request.min_reviews,
            shop: request.shop,
        }
    }

    pub fn accepts(&self, offer: &Offer) -> bool {
        self.price.contains(offer.price)
            && self.matches_words(&offer.name)
            && self.meets_quality(offer)
            && self.shop.accepts(offer)
    }

    fn matches_words(&self, name: &str) -> bool {
        let name = words(name);
        self.required.iter().all(|keyword| keyword.occurs_in(&name))
            && !self.excluded.iter().any(|keyword| keyword.occurs_in(&name))
    }

    // Неизвестный рейтинг или число отзывов условие не проходит: «рейтинг от 4.5» значит «точно не ниже 4.5».
    fn meets_quality(&self, offer: &Offer) -> bool {
        self.min_rating
            .is_none_or(|min| offer.rating.is_some_and(|rating| rating >= min))
            && self
                .min_reviews
                .is_none_or(|min| offer.reviews.is_some_and(|reviews| reviews >= min))
    }
}

/// Встречается ли слово в тексте так, как его ищет фильтр — русское без окончания, латиница и числа целиком,
/// во всех написаниях бренда.
/// По этому правилу стоп-слово убирает предложение, поэтому и подбирать стоп-слова нужно по нему же.
pub fn mentions(text: &str, word: &str) -> bool {
    let text = words(text);
    Keyword::all_in(word).iter().all(|keyword| keyword.occurs_in(&text))
}

/// Номер первого слова названия, в котором есть слово запроса, — по тому же правилу, что у фильтра.
pub fn first_mention(title_words: &[String], query: &str) -> Option<usize> {
    let keywords = Keyword::all_in(query);
    title_words
        .iter()
        .position(|word| keywords.iter().any(|keyword| keyword.is_in(word)))
}

/// Слово запроса во всех написаниях бренда: «makita» находит и «Макиту».
struct Keyword(Vec<Spelling>);

impl Keyword {
    /// Одна русская буква — предлог или союз («с», «в», «и»), а латинская буква или цифра — часть модели:
    /// Xbox Series X, Switch 2.
    fn all_in(text: &str) -> Vec<Self> {
        words(text)
            .into_iter()
            .filter(|word| word.chars().count() >= 2 || word.chars().all(|c| c.is_ascii_alphanumeric()))
            .map(|word| Self(spellings(&word).into_iter().map(Spelling::of).collect()))
            .collect()
    }

    fn occurs_in(&self, text_words: &[String]) -> bool {
        text_words.iter().any(|word| self.is_in(word))
    }

    fn is_in(&self, word: &str) -> bool {
        self.0.iter().any(|spelling| spelling.is_in(word))
    }
}

/// Русское слово склоняется — ищется без окончания: «наушники» находит и «наушников». Латиница и числа
/// не склоняются — ищутся целиком: «2» не находится в «2025», «ti» — в «Edition», «x» — в «Xbox».
struct Spelling {
    start: String,
    any_ending: bool,
}

impl Spelling {
    fn of(word: &str) -> Self {
        let any_ending = !word.ends_with(|c: char| c.is_ascii_alphanumeric());
        Self {
            start: if any_ending { stem(word) } else { word.to_string() },
            any_ending,
        }
    }

    /// С начала слова или со стыка букв и цифр: продавцы склеивают модель — «HERO13», «RTX5060TI».
    fn is_in(&self, word: &str) -> bool {
        word.char_indices()
            .filter(|&(at, c)| {
                word[..at]
                    .chars()
                    .next_back()
                    .is_none_or(|before| before.is_numeric() != c.is_numeric())
            })
            .filter_map(|(at, _)| word[at..].strip_prefix(self.start.as_str()))
            .any(|rest| self.may_be_followed_by(rest))
    }

    fn may_be_followed_by(&self, rest: &str) -> bool {
        let Some(next) = rest.chars().next() else {
            return true;
        };
        if self.start.ends_with(char::is_numeric) {
            !next.is_numeric()
        } else {
            self.any_ending || next.is_numeric()
        }
    }
}

fn stem(word: &str) -> String {
    let length = word.chars().count();
    let stem_length = if length > 5 { length - 2 } else { length };
    word.chars().take(stem_length).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(text: &str) -> OfferFilter {
        OfferFilter::new(&SearchRequest::parse(text))
    }

    fn kettle(rating: Option<f32>, reviews: Option<u64>) -> Offer {
        Offer {
            rating,
            reviews,
            ..Offer::sample("Чайник электрический", 1500)
        }
    }

    #[test]
    fn delivery_and_seller_conditions_keep_only_known_matches() {
        let offer = |delivery: Option<&str>, sold_by_ozon: bool| Offer {
            delivery: delivery.map(Into::into),
            sold_by_ozon,
            ..Offer::sample("Чайник электрический", 1_990)
        };
        let fast = filter("чайник доставка завтра");
        assert!(fast.accepts(&offer(Some("Завтра"), false)));
        assert!(!fast.accepts(&offer(Some("13 октября"), true)));
        assert!(!fast.accepts(&offer(None, true)));

        let ozon = filter("чайник только ozon");
        assert!(ozon.accepts(&offer(None, true)));
        assert!(!ozon.accepts(&offer(Some("Завтра"), false)));

        assert!(filter("чайник").accepts(&offer(None, false)));
    }

    #[test]
    fn requires_every_word_in_any_form() {
        let filter = filter("наушники sony");
        assert!(filter.accepts(&Offer::sample("Sony WH-CH520 Наушники накладные", 1)));
        assert!(filter.accepts(&Offer::sample("Чехол для наушников SONY", 1)));
        assert!(!filter.accepts(&Offer::sample("Колонка Sony", 1)));
    }

    #[test]
    fn rejects_minus_words() {
        let filter = filter("наушники sony -чехол");
        assert!(!filter.accepts(&Offer::sample("Чехол для наушников Sony", 1)));
        assert!(filter.accepts(&Offer::sample("Наушники Sony WI-C100", 1)));
    }

    #[test]
    fn respects_price_range_and_letter_yo() {
        let filter = filter("ёлка от 1000 до 2000");
        assert!(filter.accepts(&Offer::sample("Елка искусственная", 1500)));
        assert!(!filter.accepts(&Offer::sample("Ёлка искусственная", 2500)));
    }

    #[test]
    fn minimum_rating_is_inclusive_and_unknown_rating_fails() {
        let filter = filter("чайник рейтинг от 4.5");
        assert!(filter.accepts(&kettle(Some(4.5), None)));
        assert!(filter.accepts(&kettle(Some(4.9), None)));
        assert!(!filter.accepts(&kettle(Some(4.4), None)));
        assert!(!filter.accepts(&kettle(None, Some(1000))));
    }

    #[test]
    fn minimum_reviews_is_inclusive_and_unknown_count_fails() {
        let filter = filter("чайник отзывов от 100");
        assert!(filter.accepts(&kettle(None, Some(100))));
        assert!(!filter.accepts(&kettle(Some(5.0), Some(99))));
        assert!(!filter.accepts(&kettle(Some(5.0), None)));
    }

    #[test]
    fn without_quality_bounds_unrated_offers_pass() {
        assert!(filter("чайник").accepts(&kettle(None, None)));
    }

    #[test]
    fn brand_matches_in_both_alphabets() {
        let makita = filter("шуруповерт makita");
        assert!(makita.accepts(&Offer::sample("Шуруповёрт бесщеточный Макита 18 вольт", 3510)));
        assert!(makita.accepts(&Offer::sample("Шуруповерт Makita DF333", 6500)));
        assert!(!makita.accepts(&Offer::sample("Шуруповерт Bosch GSR", 6500)));

        let iphone = filter("айфон 15");
        assert!(iphone.accepts(&Offer::sample("Apple iPhone 15 128GB", 70_000)));
    }

    #[test]
    fn minus_brand_drops_both_spellings() {
        let filter = filter("наушники -самсунг");
        assert!(!filter.accepts(&Offer::sample("Наушники Samsung Galaxy Buds", 5000)));
        assert!(!filter.accepts(&Offer::sample("Наушники Самсунг", 5000)));
        assert!(filter.accepts(&Offer::sample("Наушники Sony WH-CH520", 5000)));
    }

    #[test]
    fn mentions_follows_the_same_rule_as_stop_words() {
        assert!(mentions("Наушники Logitech G435", "наушников"), "без окончания");
        assert!(mentions("Шуруповерт Makita в кейсе", "кейс"), "с окончанием");
        assert!(mentions("Шуруповерт Макита", "makita"), "в другом написании бренда");
        assert!(!mentions("Наушники Logitech G435", "чехол"));
        assert!(
            !mentions("Apple iPhone 15 128GB (PRODUCT)RED", "pro"),
            "латинское слово — целиком"
        );
    }

    fn accepts(query: &str, title: &str) -> bool {
        filter(query).accepts(&Offer::sample(title, 1))
    }

    #[test]
    fn series_x_is_not_series_s() {
        assert!(accepts(
            "xbox series x",
            "Игровая приставка Microsoft Xbox Series X 16 ГБ + 1 TБ, цифровая версия"
        ));
        assert!(!accepts(
            "xbox series x",
            "Игровая консоль Microsoft Xbox Series S 1 ТБ, белая"
        ));
    }

    #[test]
    fn switch_2_is_not_switch_oled() {
        assert!(accepts(
            "nintendo switch 2",
            "Игровая консоль Nintendo Switch 2 (глобальная версия)"
        ));
        assert!(!accepts(
            "nintendo switch 2",
            "Игровая консоль Nintendo Switch OLED, 4/64 ГБ, белая (HEG-S-KAAAA)"
        ));
    }

    #[test]
    fn ultra_2_is_not_found_in_ultra_3_of_2025() {
        assert!(accepts(
            "apple watch ultra 2",
            "Смарт-часы Apple Watch Ultra 2 49mm Titanium Case with Blue Alpine Loop, размер M (MREP3)"
        ));
        assert!(!accepts(
            "apple watch ultra 2",
            "Смарт-часы Apple Watch Ultra 3, 2025, GPS + Cellular, 49 mm, M/L (MF1H4AF/A)"
        ));
    }

    #[test]
    fn pocket_3_is_not_pocket_4() {
        assert!(accepts(
            "dji osmo pocket 3",
            "Экшн-камера DJI Osmo Pocket 3 Creator Combo"
        ));
        assert!(!accepts(
            "dji osmo pocket 3",
            "Экшн-камера DJI Osmo Pocket 4 Creator Combo"
        ));
    }

    #[test]
    fn ti_is_not_found_inside_edition() {
        assert!(accepts(
            "rtx 5060 ti",
            "Palit Видеокарта GeForce RTX 5060 Ti Dual OC 8 ГБ (NE7506TT19P1-GB2062D)"
        ));
        assert!(!accepts(
            "rtx 5060 ti",
            "ASUS Видеокарта GeForce RTX 5060 TUF Gaming OC Edition 8 ГБ (TUF-RTX5060-O8G-GAMING)"
        ));
    }

    #[test]
    fn words_glued_to_numbers_are_still_found() {
        assert!(accepts("gopro hero 13", "Экшн-камера GoPro HERO13 Black"));
        assert!(accepts("rtx 5060 ti", "Видеокарта MSI GeForce RTX5060Ti Ventus 2X"));
        assert!(accepts(
            "iphone 15 128",
            "Apple Смартфон iPhone 15 EU 6/128 ГБ, Nano-SIM, eSIM, синий"
        ));
        assert!(accepts(
            "ipad air m2",
            "Apple Планшет iPad Air 13 M2, 13\" 256 ГБ, фиолетовый"
        ));
    }

    #[test]
    fn russian_word_is_found_with_any_ending_but_not_inside_another_word() {
        assert!(accepts("робот пылесос", "Тряпки для робота-пылесоса"));
        assert!(!accepts("самокат", "Электросамокат Ninebot"));
    }
}
