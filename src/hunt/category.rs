use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::offer::Offer;
use crate::text::{normalize, words};

/// Брак и подделки в названии не участвуют в рыночной цене ни в одной категории.
const DEFECTS: &[&str] = &[
    "запчаст",
    "на детали",
    "не работ",
    "неисправ",
    "сломан",
    "разбит",
    "копия",
    "реплик",
    "муляж",
    "аналог",
];

/// В тексте объявления — только недвусмысленные фразы и целые слова. Голое «копия» поймало бы «есть копия чека»,
/// «аналог» внутри слова — «аналогов нет», а «запчаст» — продавца «Запчасти-центр».
/// «Аналог» — так на Авито подписывают подделки: «Шуруповерт Makita… Качественный аналог!».
static DEFECT_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(аналог|реплика|копия оригинала|не оригинал|на запчасти|на детали|не работает|не включается|неисправ\w*|разбит\w*|сломан\w*)\b",
    )
    .unwrap()
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Category {
    Electronics,
    Appliances,
    Tools,
    Consumables,
    Any,
}

impl Category {
    pub const ALL: [Category; 5] = [
        Category::Electronics,
        Category::Appliances,
        Category::Tools,
        Category::Consumables,
        Category::Any,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Category::Electronics => "Телефоны и электроника",
            Category::Appliances => "Бытовая техника",
            Category::Tools => "Инструменты",
            Category::Consumables => "Продукты, химия, расходники",
            Category::Any => "Всё подряд",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Category::Electronics => {
                "Не сравнивает с чехлами, стёклами, кабелями и с другими версиями модели (Pro, Max, Plus, Mini…)"
            }
            Category::Appliances => "Не сравнивает с фильтрами, мешками, насадками и другими расходниками",
            Category::Tools => {
                "Не сравнивает с битами, дисками, отдельными аккумуляторами и «тушками» без аккумулятора"
            }
            Category::Consumables => {
                "Сравнивает цену за килограмм, литр или штуку по фасовке из названия: пачка 450 г и мешок 3 кг — честно. \
                 Предложения без фасовки в названии и просроченные не участвуют"
            }
            Category::Any => "Только слова запроса и ваши стоп-слова; брак и подделки отсекаются всегда",
        }
    }

    /// Аксессуары и особые комплектации: ищутся как кусок текста в названии.
    fn accessories(self) -> &'static [&'static str] {
        match self {
            Category::Electronics => &[
                "чехол",
                "чехл",
                "защитное стекло",
                "стекло для",
                "пленк",
                "кабель",
                "переходник",
                "адаптер",
                "коробка от",
                "держатель",
                "ремешок",
                "амбушюр",
                "накладк",
                "оголовь",
                "кейс для",
                "футляр",
                "сумка для",
                "подставка",
                "наклейк",
            ],
            Category::Appliances => &[
                "фильтр для",
                "мешки для",
                "мешок для",
                "насадк",
                "щетка для",
                "шланг для",
                "запасн",
                "держатель",
            ],
            Category::Tools => &[
                "насадк",
                "бита ",
                "биты",
                "сверл",
                "диск для",
                "пилки для",
                "аккумулятор для",
                "зарядное для",
                "без акб",
                "без аккумулятор",
                "без зу",
                "тушка",
            ],
            Category::Consumables => &["просроч", "дозатор для", "диспенсер для"],
            Category::Any => &[],
        }
    }

    /// Другие версии модели: ищутся как отдельное слово, чтобы «pro» не цеплялось за «product».
    fn variants(self) -> &'static [&'static str] {
        match self {
            Category::Electronics => &["pro", "max", "plus", "mini", "ultra", "lite", "se", "fe", "air"],
            Category::Appliances | Category::Tools | Category::Consumables | Category::Any => &[],
        }
    }

    /// У продуктов и химии выгода — это цена за объём, а не за пачку. У остальных числа в названии
    /// («1,7 л» у чайника, «6 кг» у стиральной машины) — характеристики, а не фасовка.
    pub fn compares_per_unit(self) -> bool {
        self == Category::Consumables
    }

    /// Можно ли сравнивать предложение по цене с тем, что ищется: по названию и по тексту карточки.
    /// Аксессуары и версии модели видны по названию, а брак и подделки продавцы часто пишут только в тексте.
    pub fn accepts(self, query: &str, offer: &Offer) -> bool {
        let query_text = normalize(query);
        let hidden_defect = offer.description.as_deref().is_some_and(|text| {
            DEFECT_IN_TEXT
                .find_iter(&normalize(text))
                .any(|defect| !query_text.contains(defect.as_str()))
        });
        !hidden_defect && self.is_comparable(query, &offer.name)
    }

    /// Признак, который есть в самом запросе, не мешает: ищете «чехол» — чехлы и сравниваются.
    fn is_comparable(self, query: &str, title: &str) -> bool {
        let (query_text, title_text) = (normalize(query), normalize(title));
        let (query_words, title_words) = (words(query), words(title));

        let foreign_phrase = DEFECTS
            .iter()
            .chain(self.accessories())
            .any(|phrase| title_text.contains(phrase) && !query_text.contains(phrase));
        let foreign_variant = self
            .variants()
            .iter()
            .any(|variant| title_words.iter().any(|w| w == variant) && !query_words.iter().any(|w| w == variant));

        !foreign_phrase && !foreign_variant
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn electronics_ignores_accessories_and_other_versions() {
        let phones = Category::Electronics;
        assert!(phones.is_comparable("iphone 15 128", "Apple iPhone 15 128GB (PRODUCT)RED"));
        assert!(!phones.is_comparable("iphone 15 128", "Чехол для iPhone 15"));
        assert!(!phones.is_comparable("iphone 15 128", "Apple iPhone 15 Pro 128GB"));
        assert!(!phones.is_comparable("iphone 15 128", "iPhone 15 128 на запчасти"));
        assert!(phones.is_comparable("iphone 15 pro 128", "Apple iPhone 15 Pro 128GB"));
        assert!(phones.is_comparable("чехол iphone 15", "Чехол для iPhone 15 силиконовый"));
        assert!(!Category::Electronics.is_comparable(
            "наушники sony wh-ch520",
            "Накладка, оголовье для наушников Sony WH-CH520 (Бежевый)"
        ));
    }

    #[test]
    fn tools_compare_kits_with_kits_only() {
        let tools = Category::Tools;
        assert!(tools.is_comparable("шуруповерт makita", "Шуруповерт Makita DF333 с 2 АКБ"));
        assert!(!tools.is_comparable("шуруповерт makita", "Шуруповерт Makita DF333 без АКБ"));
        assert!(!tools.is_comparable("шуруповерт makita", "Шуруповерт Makita тушка"));
        assert!(tools.is_comparable("шуруповерт makita тушка", "Шуруповерт Makita тушка"));
    }

    #[test]
    fn appliances_ignore_consumables() {
        let appliances = Category::Appliances;
        assert!(appliances.is_comparable("пылесос dyson v8", "Пылесос Dyson V8 Absolute"));
        assert!(!appliances.is_comparable("пылесос dyson v8", "Фильтр для пылесоса Dyson V8"));
    }

    #[test]
    fn any_category_still_drops_defects() {
        assert!(Category::Any.is_comparable("велосипед", "Велосипед горный"));
        assert!(!Category::Any.is_comparable("велосипед", "Велосипед, разбит, на детали"));
    }

    #[test]
    fn fake_mentioned_only_in_card_text_is_dropped() {
        let listing = |text: &str| Offer {
            description: Some(text.into()),
            ..Offer::sample("Шуруповерт Makita безщеточный", 1790)
        };
        let tools = Category::Tools;
        assert!(!tools.accepts("шуруповерт makita", &listing("Лучшая цена! Качественный аналог!")));
        assert!(!tools.accepts("шуруповерт makita", &listing("Не работает подсветка")));
        assert!(tools.accepts("шуруповерт makita", &listing("В отличном состоянии, кейс, 2 АКБ")));
        assert!(tools.accepts("шуруповерт makita", &Offer::sample("Шуруповерт Makita DF333", 5000)));
    }

    #[test]
    fn honest_listing_text_is_not_mistaken_for_fake() {
        let listing = |text: &str| Offer {
            description: Some(text.into()),
            ..Offer::sample("Шуруповерт Makita DF333", 5000)
        };
        let tools = Category::Tools;
        assert!(tools.accepts("шуруповерт makita", &listing("Оригинал, есть копия чека")));
        assert!(tools.accepts("шуруповерт makita", &listing("Аналогов по цене нет")));
        assert!(tools.accepts("шуруповерт makita", &listing("Запчасти-центр. Новый, в коробке")));
        assert!(!tools.accepts("шуруповерт makita", &listing("Продаю на запчасти")));
    }
}
