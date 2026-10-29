use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};

use crate::geo::GeoPoint;
use crate::hunt::category::Category;
use crate::marketplace::{Area, Product, SourceKind};
use crate::search::{PriceRange, SearchRequest, ShopConditions, mentions};
use crate::text::words;

/// Больше — уже не подсказка, а всё название по словам.
const STOP_WORD_CANDIDATES: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Goal {
    /// Купить себе: цена ниже рынка хотя бы на столько процентов.
    BelowMarket { min_discount_percent: u8 },
    /// Перепродать: выгода после торга и расходов.
    Resale { min_profit: u64, costs: u64 },
    /// Дождаться цены одного товара по ссылке на его страницу (в `link`): не дороже `price`. Рынка тут нет —
    /// это запланированная покупка. `without_card` — сравнивать с ценой без Ozon Карты.
    TargetPrice { price: u64, without_card: bool },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub id: u64,
    pub name: String,
    pub source: SourceKind,
    pub category: Category,
    pub query: String,
    /// Ссылка на выдачу с фильтрами, настроенными на самом сайте. Ищется по ней, а слова `query` по-прежнему
    /// решают, что считать тем же товаром. Её фильтры сужают и рынок: он считается по тому, что показывает ссылка.
    #[serde(default)]
    pub link: Option<String>,
    pub price: PriceRange,
    pub stop_words: Vec<String>,
    pub min_rating: Option<f32>,
    pub min_reviews: Option<u64>,
    /// Только объявления не дальше этого от дома. Есть лишь у площадок с адресами товаров (Авито).
    #[serde(default)]
    pub radius_km: Option<u32>,
    pub goal: Goal,
    /// Дешевле половины рынка — на маркетплейсе это обычно подделка, у частников — обман.
    /// По умолчанию такие не присылаются, но и не теряются: их число видно в статусе слежки.
    #[serde(default)]
    pub include_suspicious: bool,
    /// «Доставка до завтра» (Ozon, ДНС) и «только Ozon». У Авито этих данных нет.
    /// В файлах слежек раньше называлось `ozon` — старые файлы читаются как есть.
    #[serde(default, alias = "ozon")]
    pub shop: ShopConditions,
    /// Только новые: объявления с признаками б/у не участвуют ни в рынке, ни в находках —
    /// новая и ношеная вещь разный товар. Для Авито: на Ozon всё новое.
    #[serde(default)]
    pub new_only: bool,
    pub every_minutes: u32,
    pub enabled: bool,
}

impl Watch {
    pub fn new_draft() -> Self {
        Self {
            id: 0,
            name: String::new(),
            source: SourceKind::Ozon,
            category: Category::Any,
            query: String::new(),
            link: None,
            price: PriceRange::default(),
            stop_words: Vec::new(),
            min_rating: None,
            min_reviews: None,
            radius_km: None,
            goal: Goal::BelowMarket {
                min_discount_percent: 20,
            },
            include_suspicious: false,
            shop: ShopConditions::default(),
            new_only: false,
            every_minutes: 15,
            enabled: true,
        }
    }

    /// Что не так в настройках слежки — пустой список, если всё в порядке.
    pub fn problems(&self, home_is_set: bool) -> Vec<String> {
        let mut problems = Vec::new();
        if self.name.trim().is_empty() {
            problems.push("Дайте слежке название".to_string());
        }
        if let Goal::TargetPrice { price, .. } = self.goal {
            match self.link.as_deref() {
                None => problems.push("Вставьте ссылку на страницу товара".to_string()),
                Some(link) => problems.extend(self.source.parse_product_link(link).err()),
            }
            if price == 0 {
                problems.push("Напишите цену, при которой сообщить".to_string());
            }
        } else {
            if self.query.trim().is_empty() {
                problems.push("Напишите, что искать".to_string());
            }
            if let Some(Err(problem)) = self.link.as_deref().map(|link| self.source.parse_link(link)) {
                problems.push(problem);
            }
        }
        if let (Some(min), Some(max)) = (self.price.min, self.price.max)
            && min > max
        {
            problems.push("Цена «от» больше цены «до»".to_string());
        }
        if self.every_minutes < self.source.min_interval_minutes() {
            problems.push(format!(
                "{} лучше проверять не чаще раза в {} минут — иначе начнутся проверки «я не робот»",
                self.source.title(),
                self.source.min_interval_minutes()
            ));
        }
        if matches!(self.goal, Goal::Resale { .. }) && self.radius_km.is_none() {
            problems.push(if self.source.has_locations() {
                "Для перепродажи выберите радиус от дома — забирать товар придётся самому".to_string()
            } else {
                format!(
                    "Перепродажу рядом с домом можно искать только на Авито — у {} нет адресов товаров",
                    self.source.title()
                )
            });
        }
        if let Some(radius) = self.radius_km {
            if !self.source.has_locations() {
                problems.push(format!(
                    "У {} нет адресов товаров — искать рядом с домом можно только на Авито",
                    self.source.title()
                ));
            } else if !self.source.radii_km().contains(&radius) {
                problems.push(format!("{} не умеет искать в радиусе {radius} км", self.source.title()));
            }
            if !home_is_set {
                problems.push("Чтобы искать рядом с домом, укажите его координаты в «Настройках»".to_string());
            }
        }
        problems
    }

    /// Слова из названия находки, которые можно добавить в стоп-слова: каждое уберёт её, но не сам запрос
    /// и не находки `kept`, которые остаются в ленте. Короткие слова («для», «и») и обозначения с цифрами
    /// («128gb», «2024») не предлагаются: первые встречаются в чём угодно, вторые — характеристики, а не другой товар.
    pub fn stop_word_candidates(&self, title: &str, kept: &[&str]) -> Vec<String> {
        let mut candidates: Vec<String> = Vec::new();
        for word in words(title) {
            let telling = word.chars().count() >= 4 && word.chars().all(char::is_alphabetic);
            let harmless = !mentions(&self.query, &word) && !kept.iter().any(|other| mentions(other, &word));
            let new = !self.stop_words.iter().any(|stop| mentions(&word, stop)) && !candidates.contains(&word);
            if telling && harmless && new {
                candidates.push(word);
            }
        }
        candidates.truncate(STOP_WORD_CANDIDATES);
        candidates
    }

    /// Где искать на площадке. Рынок нужен шире вашего радиуса, иначе похожих не наберётся,
    /// поэтому берётся самый большой радиус площадки, а ваш применяется к выгодным предложениям.
    pub fn search_area(&self, home: Option<GeoPoint>) -> Option<Area> {
        self.radius_km?;
        let market_radius = self.source.radii_km().iter().copied().max()?;
        home.map(|center| Area {
            center,
            radius_km: market_radius,
        })
    }

    /// Все похожие товары — по ним считается рыночная цена. Цена и рейтинг тут не важны.
    pub fn comparables_request(&self) -> SearchRequest {
        SearchRequest {
            query: self.query.clone(),
            excluded: self.stop_words.clone(),
            ..SearchRequest::default()
        }
    }

    /// Похожие товары, которые проходят ещё и ваши фильтры по цене, рейтингу, отзывам, доставке и продавцу.
    /// Рынок при этом считается по всем похожим: «только Ozon» не должно задирать рыночную цену.
    pub fn candidates_request(&self) -> SearchRequest {
        SearchRequest {
            price: self.price,
            min_rating: self.min_rating,
            min_reviews: self.min_reviews,
            shop: self.shop,
            ..self.comparables_request()
        }
    }

    /// Тот же ли рынок у слежки после правки: история рынка говорит о том, что ищется и где. Цена, рейтинг,
    /// цель и частота рынок не меняют; стоп-слова только уточняют тот же товар — их и подсказка «мимо» добавляет
    /// без перезапуска.
    pub fn same_market(&self, other: &Watch) -> bool {
        let market = |watch: &Watch| {
            (
                watch.source,
                watch.category,
                watch.query.clone(),
                watch.link.clone(),
                watch.new_only,
                watch.radius_km.is_some(),
            )
        };
        market(self) == market(other)
    }

    /// Цена, до которой ждёт слежка за товаром; у слежек по рынку её нет.
    pub fn target_price(&self) -> Option<u64> {
        match self.goal {
            Goal::TargetPrice { price, .. } => Some(price),
            Goal::BelowMarket { .. } | Goal::Resale { .. } => None,
        }
    }

    /// Цена товара, которую слежка сравнивает с целью: по умолчанию та, что Ozon показывает первой (с Ozon Картой).
    /// Без карты — цена для всех, а где она одна (ДНС) — она и есть. `None` — товара нет в продаже.
    pub fn product_price(&self, product: &Product) -> Option<u64> {
        match self.goal {
            Goal::TargetPrice { without_card: true, .. } => product.price_without_card.or(product.price),
            _ => product.price,
        }
    }

    pub fn is_due(&self, last_run: Option<DateTime<Local>>, now: DateTime<Local>) -> bool {
        self.enabled && last_run.is_none_or(|last| now - last >= Duration::minutes(i64::from(self.every_minutes)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: GeoPoint = GeoPoint { lat: 55.75, lon: 37.62 };

    fn watch() -> Watch {
        Watch {
            name: "Айфон".into(),
            query: "iphone 15 128".into(),
            ..Watch::new_draft()
        }
    }

    fn drill_for_resale() -> Watch {
        Watch {
            name: "Шуруповёрт".into(),
            query: "шуруповерт makita".into(),
            source: SourceKind::Avito,
            radius_km: Some(5),
            goal: Goal::Resale {
                min_profit: 2000,
                costs: 500,
            },
            ..Watch::new_draft()
        }
    }

    #[test]
    fn filled_watch_has_no_problems() {
        assert!(watch().problems(false).is_empty());
        assert!(drill_for_resale().problems(true).is_empty());
    }

    #[test]
    fn empty_and_contradictory_fields_are_reported() {
        let broken = Watch {
            name: " ".into(),
            query: String::new(),
            price: PriceRange {
                min: Some(5000),
                max: Some(1000),
            },
            every_minutes: 1,
            ..watch()
        };
        assert_eq!(broken.problems(true).len(), 4);
    }

    #[test]
    fn link_must_lead_to_the_watch_source() {
        let by_link = |link: &str| Watch {
            link: Some(link.into()),
            ..watch()
        };
        assert!(
            by_link("https://www.ozon.ru/category/audiotehnika-15543/?brand=32686750")
                .problems(false)
                .is_empty()
        );
        assert_eq!(
            by_link("https://www.avito.ru/moskva?q=iphone").problems(false),
            ["Ссылка должна вести на www.ozon.ru, а эта ведёт на www.avito.ru"]
        );
    }

    fn product_watch(link: Option<&str>, price: u64) -> Watch {
        Watch {
            name: "Наушники к зиме".into(),
            link: link.map(Into::into),
            goal: Goal::TargetPrice {
                price,
                without_card: false,
            },
            ..Watch::new_draft()
        }
    }

    #[test]
    fn product_watch_needs_a_product_link_and_a_price_but_no_query() {
        let link = "https://www.ozon.ru/product/sony-wh-ch520-895366703/";
        assert!(product_watch(Some(link), 3_500).problems(false).is_empty());
        assert_eq!(
            product_watch(None, 0).problems(false),
            [
                "Вставьте ссылку на страницу товара",
                "Напишите цену, при которой сообщить"
            ]
        );
        let search_link = product_watch(Some("https://www.ozon.ru/search/?text=sony"), 3_500).problems(false);
        assert!(search_link[0].starts_with("Это ссылка не на товар"), "{search_link:?}");
    }

    #[test]
    fn product_price_is_with_ozon_card_unless_asked_without() {
        let product = Product {
            name: "Sony WH-CH520".into(),
            price: Some(3_870),
            price_without_card: Some(3_990),
        };
        let with_card = product_watch(None, 3_500);
        let without_card = Watch {
            goal: Goal::TargetPrice {
                price: 3_500,
                without_card: true,
            },
            ..with_card.clone()
        };
        assert_eq!(with_card.product_price(&product), Some(3_870));
        assert_eq!(without_card.product_price(&product), Some(3_990));

        let one_price = Product {
            price_without_card: None,
            ..product.clone()
        };
        assert_eq!(without_card.product_price(&one_price), Some(3_870), "у ДНС цена одна");
        let sold_out = Product {
            price: None,
            price_without_card: None,
            ..product
        };
        assert_eq!(without_card.product_price(&sold_out), None);
    }

    #[test]
    fn stop_word_candidates_drop_the_offer_but_not_the_query() {
        let headphones = Watch {
            query: "наушники sony wh-ch520".into(),
            stop_words: vec!["копия".into()],
            ..watch()
        };

        assert_eq!(
            headphones.stop_word_candidates("Чехол для наушников Сони WH-CH520, силиконовый, 2024", &[]),
            ["чехол", "силиконовый"],
            "«наушников» и «Сони» — это сам запрос, «для» и «2024» ничего не говорят"
        );
        assert_eq!(
            headphones.stop_word_candidates(
                "Беспроводные наушники Sony WH-CH520 копия, бежевые",
                &["Беспроводные наушники Sony WH-CH520, синие"]
            ),
            ["бежевые"],
            "«копия» — уже стоп-слово, «беспроводные» убрали бы и хорошую находку"
        );
    }

    #[test]
    fn watches_saved_before_links_have_none() {
        let mut saved = serde_json::to_value(watch()).unwrap();
        saved.as_object_mut().unwrap().remove("link");

        let read: Watch = serde_json::from_value(saved).unwrap();

        assert_eq!(read.link, None);
    }

    #[test]
    fn nearby_search_needs_locations_supported_radius_and_home() {
        let on_ozon = Watch {
            source: SourceKind::Ozon,
            ..drill_for_resale()
        };
        assert!(on_ozon.problems(true).iter().any(|p| p.contains("только на Авито")));

        let odd_radius = Watch {
            radius_km: Some(30),
            ..drill_for_resale()
        };
        assert!(
            odd_radius
                .problems(true)
                .iter()
                .any(|p| p.contains("не умеет искать в радиусе 30 км"))
        );

        assert!(
            drill_for_resale()
                .problems(false)
                .iter()
                .any(|p| p.contains("координаты"))
        );
    }

    #[test]
    fn resale_without_radius_is_rejected() {
        let anywhere = Watch {
            radius_km: None,
            ..drill_for_resale()
        };
        assert!(anywhere.problems(true).iter().any(|p| p.contains("выберите радиус")));

        let ozon_resale = Watch {
            source: SourceKind::Ozon,
            ..anywhere
        };
        let problems = ozon_resale.problems(true);
        assert!(problems.iter().any(|p| p.contains("только на Авито")));
        assert!(!problems.iter().any(|p| p.contains("выберите радиус")));
    }

    #[test]
    fn market_is_searched_in_widest_radius_around_home() {
        assert_eq!(
            drill_for_resale().search_area(Some(HOME)),
            Some(Area {
                center: HOME,
                radius_km: 25
            })
        );
        assert_eq!(drill_for_resale().search_area(None), None);
        let anywhere = Watch {
            radius_km: None,
            goal: Goal::BelowMarket {
                min_discount_percent: 20,
            },
            ..drill_for_resale()
        };
        assert_eq!(anywhere.search_area(Some(HOME)), None);
    }

    #[test]
    fn requests_split_market_from_your_filters() {
        let picky = Watch {
            price: PriceRange {
                min: None,
                max: Some(60_000),
            },
            min_rating: Some(4.5),
            stop_words: vec!["восстановленный".into()],
            ..watch()
        };
        assert_eq!(picky.comparables_request().price, PriceRange::default());
        assert_eq!(picky.comparables_request().min_rating, None);
        assert_eq!(picky.candidates_request().price.max, Some(60_000));
        assert_eq!(picky.candidates_request().excluded, ["восстановленный"]);
    }

    #[test]
    fn market_changes_with_what_is_searched_not_with_your_filters() {
        let stricter = Watch {
            price: PriceRange {
                min: None,
                max: Some(60_000),
            },
            goal: Goal::BelowMarket {
                min_discount_percent: 30,
            },
            stop_words: vec!["восстановленный".into()],
            every_minutes: 60,
            ..watch()
        };
        assert!(watch().same_market(&stricter));

        let other_query = Watch {
            query: "iphone 16 128".into(),
            ..watch()
        };
        let by_link = Watch {
            link: Some("https://www.ozon.ru/category/smartfony-15502/?delivery=2".into()),
            ..watch()
        };
        assert!(!watch().same_market(&other_query));
        assert!(!watch().same_market(&by_link));

        let anywhere = Watch {
            radius_km: None,
            ..drill_for_resale()
        };
        let farther = Watch {
            radius_km: Some(10),
            ..drill_for_resale()
        };
        assert!(
            !drill_for_resale().same_market(&anywhere),
            "рядом с домом рынок ищется вокруг дома"
        );
        assert!(
            drill_for_resale().same_market(&farther),
            "а вокруг дома — всегда в самом большом радиусе"
        );
    }

    #[test]
    fn due_after_interval_and_only_when_enabled() {
        let now = Local::now();
        let watch = watch();
        assert!(watch.is_due(None, now));
        assert!(!watch.is_due(Some(now - Duration::minutes(14)), now));
        assert!(watch.is_due(Some(now - Duration::minutes(15)), now));
        assert!(
            !Watch {
                enabled: false,
                ..watch
            }
            .is_due(None, now)
        );
    }
}
