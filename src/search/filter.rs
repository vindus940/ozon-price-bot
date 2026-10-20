use crate::offer::Offer;
use crate::search::synonyms::spellings;
use crate::search::{PriceRange, SearchRequest, ShopConditions};
use crate::text::{normalize, words};

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
        let name = normalize(name);
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

/// Встречается ли слово в тексте так, как его ищет фильтр — без окончания и во всех написаниях бренда.
/// По этому правилу стоп-слово убирает предложение, поэтому и подбирать стоп-слова нужно по нему же.
pub fn mentions(text: &str, word: &str) -> bool {
    let text = normalize(text);
    Keyword::all_in(word).iter().all(|keyword| keyword.occurs_in(&text))
}

/// Слово запроса без окончания и во всех написаниях бренда: «наушники» находит и «наушников»,
/// «makita» — и «Макиту».
struct Keyword(Vec<String>);

impl Keyword {
    fn all_in(text: &str) -> Vec<Self> {
        words(text)
            .into_iter()
            .filter(|word| word.chars().count() >= 2)
            .map(|word| Self(spellings(&word).into_iter().map(stem).collect()))
            .collect()
    }

    fn occurs_in(&self, normalized_text: &str) -> bool {
        self.0.iter().any(|stem| normalized_text.contains(stem.as_str()))
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
        assert!(mentions("Шуруповерт Макита", "makita"), "в другом написании бренда");
        assert!(!mentions("Наушники Logitech G435", "чехол"));
    }
}
