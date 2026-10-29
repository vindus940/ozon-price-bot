use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::offer::Offer;
use crate::search::first_mention;
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
    "подделк",
    "фейк",
    "1:1",
    "1в1",
    "некомплект",
    "не комплект",
    "с дефект",
];

/// В тексте объявления — только недвусмысленные фразы и целые слова. Голое «копия» поймало бы «есть копия чека»,
/// «аналог» внутри слова — «аналогов нет», «запчаст» — продавца «Запчасти-центр», а «дефект» — «без дефектов».
/// «Аналог» — так на Авито подписывают подделки: «Шуруповерт Makita… Качественный аналог!».
static DEFECT_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(аналог|реплика|копия оригинала|не оригинал\w*|подделк\w*|фейк\w*|1\s?[:в]\s?1|на запчасти|на детали|под ремонт|требует ремонт\w*|не работает|не включается|неисправ\w*|разбит\w*|сломан\w*|с дефект\w*|есть дефект\w*|пропадает звук|не ?комплект\w*)\b",
    )
    .unwrap()
});

/// Признаки б/у в тексте объявления. Сам Авито состояние в выдаче не пишет, а продавцы — почти всегда:
/// «б/у», «пользовался год», «следы использования», «состояние 9/10», «купил другие». «Купил новые» — тоже б/у:
/// поэтому новизну не угадываем по слову «новые», а только отсеиваем явно ношеное.
static USED_IN_TEXT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:^|[^\p{L}])(б/у|б\.у\.|был[аи]? в (?:употреблении|использовании)|в использовании|пользовал(?:ся|ась|ись)|использовал\w*|следы? (?:и )?(?:использования|эксплуатации|пользования)|следами|износ\w*|изношен\w*|потерт\w*|потёрт\w*|царапин\w*|состояни\w* (?:отличн|хорош|идеальн|нормальн|удовлетворительн)\w*|(?:отличн|хорош|идеальн)\w* состояни\w*|\d{1,2} ?/ ?10|купил[аи]? (?:другие|другой|другую|новые|новый|новую))",
    )
    .unwrap()
});

/// Похоже на б/у по названию или тексту объявления.
pub fn looks_used(offer: &Offer) -> bool {
    let text = normalize(&format!(
        "{} {}",
        offer.name,
        offer.description.as_deref().unwrap_or_default()
    ));
    USED_IN_TEXT.is_match(&text)
}

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
            Category::Appliances => {
                "Не сравнивает с фильтрами, мешками, насадками, другими расходниками и с другими версиями модели \
                 (Pro, Plus, Ultra…)"
            }
            Category::Tools => {
                "Не сравнивает с битами, дисками, отдельными аккумуляторами и «тушками» без аккумулятора"
            }
            Category::Consumables => {
                "Сравнивает цену за килограмм, литр или штуку по фасовке из названия: пачка 450 г и мешок 3 кг — честно. \
                 Предложения без фасовки в названии и просроченные не участвуют"
            }
            Category::Any => {
                "Только слова запроса и ваши стоп-слова; брак, подделки и вещи «для» искомого отсекаются всегда"
            }
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
                "аксессуар",
                "аккумулятор для",
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
                "аксессуар",
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
            Category::Electronics => &["pro", "max", "plus", "mini", "ultra", "lite", "se", "fe", "air", "ti"],
            Category::Appliances => &["pro", "max", "plus", "mini", "ultra", "lite"],
            Category::Tools | Category::Consumables | Category::Any => &[],
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

        !foreign_phrase && !foreign_variant && !made_for(query, &title_words)
    }
}

/// «Руль … (для Xbox Series X/S)», «Аккумулятор … для GOPRO HERO 13»: искомое названо сразу после «для» —
/// значит, это вещь для него, в любой категории. «Фен для волос Dyson» — сам фен: после «для» идёт не запрос;
/// «Чехол для iPhone» по запросу «чехол iphone» — сам чехол: «чехол» из запроса стоит раньше «для».
fn made_for(query: &str, title_words: &[String]) -> bool {
    first_mention(title_words, query).is_some_and(|first| title_words[..first].last().is_some_and(|word| word == "для"))
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
    fn robot_vacuum_is_not_its_accessories_or_other_versions() {
        let appliances = Category::Appliances;
        assert!(appliances.is_comparable("roborock s8", "Робот-пылесос Roborock S8, Black (S80ULT)"));
        for title in [
            "Тряпки из микрофибры для робота пылесоса Roborock S7 MaxV S8 Plus T7s G10",
            "Комплект фильтров и щеток для робота-пылесоса Roborock S8, S8 Plus, S8 Pro Ultra (чёрные щётки)",
            "Аксессуары для роботов-пылесосов Roborock S8 Pro / Roborock S9 Plus.(фильтры, боковые щётки, салфетки)",
            "Подарочный набор аксессуаров для Roborock S8 Max Ultra / S8 / S8 Plus: моющиеся фильтры 2",
            "Набор аксессуаров к роботу-пылесосу Roborock S8",
            "Робот-пылесос Roborock S8 Pro Ultra, белый",
        ] {
            assert!(!appliances.is_comparable("roborock s8", title), "{title}");
        }
    }

    #[test]
    fn wheel_for_xbox_is_not_xbox() {
        let electronics = Category::Electronics;
        assert!(!electronics.is_comparable(
            "xbox series x",
            "Руль Logitech G923 TRUEFORCE (для Xbox Series X/S/One/PC)"
        ));
        assert!(!electronics.is_comparable(
            "xbox series x",
            "Аккумулятор для геймпада Xbox Series X/Series S/ Xbox One с кабелем зарядки Type-C"
        ));
    }

    #[test]
    fn things_for_gopro_are_not_gopro_in_any_category() {
        let electronics = Category::Electronics;
        assert!(electronics.is_comparable(
            "gopro hero 13",
            "GoPro Экшн-камера GoPro Hero 13 Black, черный матовый, черный"
        ));
        assert!(!electronics.is_comparable(
            "gopro hero 13",
            "K&F CONCEPT CPL светофильтр для GoPro Hero 9/10/11/12/13 поляризационный, AGC оптическое стекло"
        ));
        assert!(!Category::Any.is_comparable("gopro hero 13", "Аккумулятор Kingma для GoPro HERO 13, 1900 мАч"));
        assert!(!electronics.is_comparable("gopro hero 13", "Набор аксессуаров для экшн-камеры GoPro HERO 13 Black"));
    }

    #[test]
    fn purpose_after_for_is_not_an_accessory() {
        assert!(
            Category::Appliances.is_comparable("dyson airwrap", "Стайлер для волос Dyson Airwrap Complete Long HS05")
        );
        assert!(Category::Consumables.is_comparable("persil", "Гель для стирки Persil 1 л"));
        assert!(Category::Electronics.is_comparable("чехол iphone 15", "Чехол для iPhone 15 силиконовый"));
    }

    #[test]
    fn graphics_card_without_ti_is_not_ti() {
        assert!(!Category::Electronics.is_comparable(
            "rtx 5060",
            "Palit Видеокарта GeForce RTX 5060 Ti Dual OC 8 ГБ (NE7506TT19P1-GB2062D)"
        ));
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

    fn headphones(text: &str) -> Offer {
        Offer {
            description: Some(text.into()),
            ..Offer::sample("Наушники Logitech G435", 1_700)
        }
    }

    #[test]
    fn defects_and_incomplete_sets_from_real_listings_are_dropped() {
        let any = Category::Any;
        for text in [
            "Левый наушник с дефектом, переодически пропадает звук.",
            "Оригинал. Не комплектные(нет проводов и донгла).",
            "Копия 1:1, звук как у оригинала",
            "Продаю под ремонт",
        ] {
            assert!(!any.accepts("logitech g435", &headphones(text)), "{text}");
        }
        assert!(any.accepts("logitech g435", &headphones("Без дефектов, полный комплект")));
    }

    #[test]
    fn used_is_recognized_by_what_sellers_actually_write() {
        for text in [
            "После покупки пользовался около года",
            "Продаю игровую гарнитуру. Состояние отличное, всё работает",
            "Оригинал, состояние 9/10, из минусов только амбушюры",
            "Хорошие наушники. Продаю т. к. купил другие",
            "Наушники со следами и пользования",
            "Беспроводные наушники Logitech 435 б/у",
        ] {
            assert!(looks_used(&headphones(text)), "{text}");
        }
        for text in ["Новые, в упаковке, не вскрывались", "Состояние: новое. Чек есть", ""]
        {
            assert!(!looks_used(&headphones(text)), "{text}");
        }
    }
}
