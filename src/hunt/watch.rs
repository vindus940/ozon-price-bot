use chrono::{DateTime, Duration, Local, Timelike};
use serde::{Deserialize, Serialize};

use crate::geo::GeoPoint;
use crate::hunt::category::Category;
use crate::marketplace::{Area, Product, Scope, SourceKind};
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

/// У кого искать: у всех продавцов или только у частных (`SourceKind::has_private_sellers`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sellers {
    #[default]
    Any,
    Private,
}

/// Часы, в которые слежка проверяет площадку: с `from` часов включительно до `to` часов. `from` больше `to` —
/// через полночь (с 22 до 7). Меньше заходов на сайты ночью — и ни одного уведомления, пока вы спите.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hours {
    pub from: u32,
    pub to: u32,
}

impl Hours {
    pub fn contains(self, at: DateTime<Local>) -> bool {
        let hour = at.hour();
        if self.from <= self.to {
            (self.from..self.to).contains(&hour)
        } else {
            hour >= self.from || hour < self.to
        }
    }

    /// Ближайшее начало часов не раньше `at`.
    pub fn next_start(self, at: DateTime<Local>) -> DateTime<Local> {
        let start_on = |day: chrono::NaiveDate| {
            day.and_hms_opt(self.from, 0, 0)
                .and_then(|start| start.and_local_timezone(Local).earliest())
        };
        let today = start_on(at.date_naive()).filter(|&start| start >= at);
        today
            .or_else(|| start_on(at.date_naive() + Duration::days(1)))
            .unwrap_or(at)
    }
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
    /// Только частные продавцы — без магазинов и перекупов (Авито). Это фильтр самого сайта, поэтому рынок тоже
    /// считается по частным: с ними и придётся торговаться, и им же — продавать.
    #[serde(default)]
    pub sellers: Sellers,
    pub every_minutes: u32,
    /// Когда проверять; `None` — круглосуточно (так читаются и слежки, сохранённые до появления часов).
    /// «Проверить сейчас» и пробная проверка работают в любое время.
    #[serde(default)]
    pub hours: Option<Hours>,
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
            sellers: Sellers::Any,
            every_minutes: 15,
            hours: None,
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
        if let Some(hours) = self.hours {
            if hours.from == hours.to {
                problems.push("Часы «с» и «до» совпадают — уберите галочку, если проверять круглосуточно".to_string());
            }
            if hours.from > 23 || hours.to > 23 {
                problems.push("Часы — от 0 до 23".to_string());
            }
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

    /// Где и у кого искать на площадке: область вокруг дома и «только частные».
    pub fn scope(&self, home: Option<GeoPoint>) -> Scope {
        Scope {
            area: self.search_area(home),
            private_only: self.sellers == Sellers::Private && self.source.has_private_sellers(),
        }
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
    /// без перезапуска. У слежки за товаром история — его цена: другая цена (с картой или без) — другой ряд,
    /// а сменить «рынок» на «товар» — совсем другая история.
    pub fn same_market(&self, other: &Watch) -> bool {
        let market = |watch: &Watch| {
            let product_price = match watch.goal {
                Goal::TargetPrice { without_card, .. } => Some(without_card),
                Goal::BelowMarket { .. } | Goal::Resale { .. } => None,
            };
            (
                watch.source,
                watch.category,
                watch.query.clone(),
                watch.link.clone(),
                watch.new_only,
                watch.sellers,
                watch.radius_km.is_some(),
                product_price,
            )
        };
        market(self) == market(other)
    }

    /// Где искать выгодное глубже популярного: от половины рынка (дешевле — подделка или обман, ниже идут уже чехлы
    /// и мелочь) до рынка минус нужная скидка, в пределах вашей цены «от–до». Только для «ниже рынка»: у расходников
    /// рынок — за килограмм, коридор в рублях за пачку ничего не значит, а перепродажа ищется рядом с домом на Авито.
    pub fn cheapest_range(&self, market: u64) -> Option<(u64, u64)> {
        let Goal::BelowMarket { min_discount_percent } = self.goal else {
            return None;
        };
        if self.category.compares_per_unit() {
            return None;
        }
        let from = (market / 2).max(self.price.min.unwrap_or(0));
        let to = (market * (100 - u64::from(min_discount_percent)) / 100).min(self.price.max.unwrap_or(u64::MAX));
        (from > 0 && from <= to).then_some((from, to))
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
        self.enabled
            && self.hours.is_none_or(|hours| hours.contains(now))
            && last_run.is_none_or(|last| now - last >= Duration::minutes(i64::from(self.every_minutes)))
    }

    /// Когда слежка проверит площадку после проверки в `last_run`: через свой интервал, а выпадет на нерабочие
    /// часы — в начале следующих.
    pub fn next_check(&self, last_run: DateTime<Local>) -> DateTime<Local> {
        let due = last_run + Duration::minutes(i64::from(self.every_minutes));
        match self.hours {
            Some(hours) if !hours.contains(due) => hours.next_start(due),
            _ => due,
        }
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
    fn cheapest_range_is_from_half_the_market_to_the_wanted_discount_within_own_prices() {
        let headphones = Watch {
            goal: Goal::BelowMarket {
                min_discount_percent: 20,
            },
            ..watch()
        };
        assert_eq!(headphones.cheapest_range(4_400), Some((2_200, 3_520)));

        let up_to_3000 = Watch {
            price: PriceRange {
                min: Some(2_500),
                max: Some(3_000),
            },
            ..headphones.clone()
        };
        assert_eq!(up_to_3000.cheapest_range(4_400), Some((2_500, 3_000)));
        let too_cheap = Watch {
            price: PriceRange {
                min: None,
                max: Some(2_000),
            },
            ..headphones.clone()
        };
        assert_eq!(
            too_cheap.cheapest_range(4_400),
            None,
            "всё, что дешевле 2 000, — подозрительно"
        );

        let powder = Watch {
            category: Category::Consumables,
            ..headphones
        };
        assert_eq!(powder.cheapest_range(1_000), None, "у расходников рынок за килограмм");
        assert_eq!(drill_for_resale().cheapest_range(10_000), None);
    }

    #[test]
    fn product_price_history_survives_a_new_target_but_not_another_price_or_goal() {
        let link = "https://www.ozon.ru/product/sony-wh-ch520-895366703/";
        let headphones = product_watch(Some(link), 3_500);
        let cheaper_target = product_watch(Some(link), 3_000);
        let without_card = Watch {
            goal: Goal::TargetPrice {
                price: 3_500,
                without_card: true,
            },
            ..headphones.clone()
        };
        let by_market = Watch {
            goal: Goal::BelowMarket {
                min_discount_percent: 20,
            },
            ..headphones.clone()
        };
        assert!(headphones.same_market(&cheaper_target));
        assert!(!headphones.same_market(&without_card), "цена без карты — другой ряд");
        assert!(!headphones.same_market(&by_market));
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

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        use chrono::TimeZone;
        Local.with_ymd_and_hms(2026, 10, day, hour, minute, 0).unwrap()
    }

    #[test]
    fn hours_include_the_start_exclude_the_end_and_may_cross_midnight() {
        let day = Hours { from: 8, to: 23 };
        assert!(!day.contains(at(29, 7, 59)));
        assert!(day.contains(at(29, 8, 0)));
        assert!(day.contains(at(29, 22, 59)));
        assert!(!day.contains(at(29, 23, 0)));

        let night = Hours { from: 22, to: 7 };
        assert!(night.contains(at(29, 23, 30)));
        assert!(night.contains(at(30, 6, 59)));
        assert!(!night.contains(at(30, 7, 0)));
        assert!(!night.contains(at(29, 12, 0)));
    }

    #[test]
    fn watch_outside_its_hours_waits_and_names_the_next_start() {
        let daytime = Watch {
            hours: Some(Hours { from: 8, to: 23 }),
            every_minutes: 30,
            ..watch()
        };
        assert!(
            !daytime.is_due(None, at(29, 23, 10)),
            "ночью не проверяет даже ни разу не проверенную"
        );
        assert!(daytime.is_due(Some(at(29, 22, 40)), at(30, 8, 0)));

        assert_eq!(daytime.next_check(at(29, 22, 0)), at(29, 22, 30));
        assert_eq!(
            daytime.next_check(at(29, 22, 40)),
            at(30, 8, 0),
            "23:10 — уже нерабочее время"
        );
        assert_eq!(
            daytime.next_check(at(30, 7, 0)),
            at(30, 8, 0),
            "рано утром — с начала часов того же дня"
        );
        assert_eq!(
            watch().next_check(at(29, 22, 40)),
            at(29, 22, 55),
            "круглосуточная — просто через интервал"
        );
    }

    #[test]
    fn same_or_impossible_hours_are_reported_and_old_watches_run_around_the_clock() {
        let same = Watch {
            hours: Some(Hours { from: 9, to: 9 }),
            ..watch()
        };
        assert!(same.problems(false)[0].starts_with("Часы «с» и «до» совпадают"));
        let impossible = Watch {
            hours: Some(Hours { from: 8, to: 24 }),
            ..watch()
        };
        assert_eq!(impossible.problems(false), ["Часы — от 0 до 23"]);

        let mut saved = serde_json::to_value(watch()).unwrap();
        saved.as_object_mut().unwrap().remove("hours");
        saved.as_object_mut().unwrap().remove("sellers");
        let old: Watch = serde_json::from_value(saved).unwrap();
        assert_eq!(
            (old.hours, old.sellers),
            (None, Sellers::Any),
            "старые слежки — круглосуточно и у всех продавцов"
        );
        let private = Watch {
            sellers: Sellers::Private,
            ..watch()
        };
        assert!(!watch().same_market(&private), "рынок частных — другой рынок");
    }
}
