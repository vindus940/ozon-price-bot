use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};

use crate::geo::GeoPoint;
use crate::hunt::category::Category;
use crate::marketplace::{Area, SourceKind};
use crate::search::{PriceRange, SearchRequest};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Goal {
    /// Купить себе: цена ниже рынка хотя бы на столько процентов.
    BelowMarket { min_discount_percent: u8 },
    /// Перепродать: выгода после торга и расходов.
    Resale { min_profit: u64, costs: u64 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Watch {
    pub id: u64,
    pub name: String,
    pub source: SourceKind,
    pub category: Category,
    pub query: String,
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
            price: PriceRange::default(),
            stop_words: Vec::new(),
            min_rating: None,
            min_reviews: None,
            radius_km: None,
            goal: Goal::BelowMarket {
                min_discount_percent: 20,
            },
            include_suspicious: false,
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
        if self.query.trim().is_empty() {
            problems.push("Напишите, что искать".to_string());
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

    /// Похожие товары, которые проходят ещё и ваши фильтры по цене, рейтингу и отзывам.
    pub fn candidates_request(&self) -> SearchRequest {
        SearchRequest {
            price: self.price,
            min_rating: self.min_rating,
            min_reviews: self.min_reviews,
            ..self.comparables_request()
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
