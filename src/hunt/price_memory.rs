use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Local};
use serde::{Deserialize, Serialize};

use crate::format::rub;
use crate::hunt::category::Category;
use crate::marketplace::SourceKind;
use crate::search::SearchRequest;
use crate::storage::Storage;
use crate::text::words;

const FILE: &str = "best-prices.json";

/// Столько дней помнится лучшая цена запроса — как и история рынка у слежек.
const KEPT_DAYS: i64 = 30;
/// Точка в час: поиски подряд не должны растягивать файл, а самая низкая цена часа от этого не теряется.
const POINT_EVERY_MINUTES: i64 = 60;
/// В пределах этого от минимума цена уже хороша: ждать ещё нескольких процентов обычно незачем.
const NEAR_THE_LOW_PERCENT: u64 = 5;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct PricePoint {
    at: DateTime<Local>,
    price: u64,
    source: SourceKind,
    /// Запрос, как его написали: в ключе слова отсортированы, а `wh-ch520` разбито и как запрос уже не годится.
    #[serde(default)]
    asked: String,
}

/// Лучшая цена прошлых поисков того же запроса.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PastLow {
    pub price: u64,
    pub source: SourceKind,
    pub at: DateTime<Local>,
    /// С какого поиска есть история.
    pub since: DateTime<Local>,
}

/// Прошлый запрос для `/history`.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchedQuery {
    pub query: String,
    pub source: SourceKind,
    pub price: u64,
    pub lowest: u64,
    pub at: DateTime<Local>,
}

impl PastLow {
    /// Что сказать о цене `price` рядом с лучшей ценой прошлых поисков.
    pub fn says(&self, price: u64) -> String {
        let since = self.since.format("%d.%m");
        let low = format!(
            "{} {} ({})",
            self.source.title(),
            rub(self.price),
            self.at.format("%d.%m")
        );
        if price <= self.price {
            format!("Самая низкая цена с {since}")
        } else if price * 100 <= self.price * (100 + NEAR_THE_LOW_PERCENT) {
            format!("Почти самая низкая с {since}: было {low}")
        } else {
            format!("С {since} бывало дешевле: {low}")
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(transparent)]
struct Points(HashMap<String, Vec<PricePoint>>);

impl Points {
    /// Сначала — что было раньше, потом запись новой цены: новая цена с собой не сравнивается.
    fn note(&mut self, key: String, point: PricePoint) -> Option<PastLow> {
        let now = point.at;
        let points = self.0.entry(key).or_default();
        points.retain(|point| now - point.at < Duration::days(KEPT_DAYS));
        let low = points.iter().min_by_key(|point| point.price).map(|low| PastLow {
            price: low.price,
            source: low.source,
            at: low.at,
            since: points[0].at,
        });
        Self::add(points, point);
        low
    }

    fn add(points: &mut Vec<PricePoint>, point: PricePoint) {
        match points.last_mut() {
            Some(last) if point.at - last.at < Duration::minutes(POINT_EVERY_MINUTES) => {
                if point.price < last.price {
                    *last = PricePoint { at: last.at, ..point };
                }
            }
            _ => points.push(point),
        }
    }
}

/// Память лучшей цены разовых поисков: окно и бот ищут одно и то же, поэтому она общая.
/// Заходов на сайты не добавляет: запоминается то, что поиск и так нашёл.
pub struct PriceMemory {
    storage: Storage,
    points: Mutex<Points>,
}

impl PriceMemory {
    pub fn new(storage: Storage) -> Self {
        let points = Mutex::new(storage.load(FILE));
        Self { storage, points }
    }

    /// Запомнить лучшую цену запроса и сказать, какая была раньше. Запрос с условиями о цене и магазине — другой вопрос,
    /// и сравнивать его с простым нельзя, поэтому он не запоминается. Другая категория — тоже: в «Всё подряд» лучшей
    /// ценой может быть то, что категория «Электроника» отсеяла бы как аксессуар.
    pub fn note(
        &self,
        request: &SearchRequest,
        category: Category,
        source: SourceKind,
        price: u64,
        now: DateTime<Local>,
    ) -> Option<PastLow> {
        let key = key_of(request, category)?;
        let mut points = self.points.lock().unwrap();
        let point = PricePoint {
            at: now,
            price,
            source,
            asked: request.query.clone(),
        };
        let low = points.note(key, point);
        if let Err(error) = self.storage.save(FILE, &*points) {
            log::warn!("лучшая цена не записана: {error:#}");
        }
        low
    }

    /// Последние запросы, новые первыми. Один запрос в разных категориях — одна строка.
    pub fn recent(&self, limit: usize) -> Vec<SearchedQuery> {
        let points = self.points.lock().unwrap();
        let mut queries: Vec<SearchedQuery> = points
            .0
            .values()
            .filter_map(|points| {
                let last = points.last()?;
                Some(SearchedQuery {
                    query: last.asked.clone(),
                    source: last.source,
                    price: last.price,
                    lowest: points.iter().map(|point| point.price).min()?,
                    at: last.at,
                })
            })
            .collect();
        queries.sort_by_key(|searched| Reverse(searched.at));
        let mut seen = HashSet::new();
        queries.retain(|searched| seen.insert(searched.query.clone()));
        queries.truncate(limit);
        queries
    }

    #[cfg(test)]
    fn start(&self, key: &str, price: u64, source: SourceKind, at: DateTime<Local>) {
        let mut points = self.points.lock().unwrap();
        points.0.entry(key.to_string()).or_default().push(PricePoint {
            at,
            price,
            source,
            asked: key.split_once(": ").map_or(key, |(_, asked)| asked).to_string(),
        });
    }
}

/// Порядок слов и регистр вопрос не меняют: «Sony наушники» и «наушники sony» — один запрос.
fn key_of(request: &SearchRequest, category: Category) -> Option<String> {
    let plain = *request
        == SearchRequest {
            query: request.query.clone(),
            ..SearchRequest::default()
        };
    let mut words = words(&request.query);
    words.sort();
    (plain && !words.is_empty()).then(|| format!("{category:?}: {}", words.join(" ")))
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn memory(test: &str) -> (PriceMemory, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("ozon-price-bot-memory-{test}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        (PriceMemory::new(Storage::new(dir.clone())), dir)
    }

    fn point(price: u64, source: SourceKind, at: DateTime<Local>) -> PricePoint {
        PricePoint {
            at,
            price,
            source,
            asked: "запрос".into(),
        }
    }

    fn day(day: u32, hour: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 11, day, hour, 0, 0).unwrap()
    }

    const KEY: &str = "Any: sony наушники";

    fn sony() -> SearchRequest {
        SearchRequest::parse("наушники sony")
    }

    #[test]
    fn first_search_has_no_past_and_the_next_one_sees_the_lowest() {
        let (memory, dir) = memory("lowest");
        let mut first = Points::default();
        assert_eq!(first.note("a".into(), point(100, SourceKind::Ozon, day(1, 10))), None);

        memory.start(KEY, 3_000, SourceKind::Ozon, day(1, 10));
        memory.start(KEY, 2_500, SourceKind::Wildberries, day(2, 10));
        memory.start(KEY, 2_800, SourceKind::Ozon, day(3, 10));

        let low = memory.note(
            &SearchRequest::parse("Sony наушники"),
            Category::Any,
            SourceKind::Dns,
            2_900,
            day(4, 10),
        );

        assert_eq!(
            low,
            Some(PastLow {
                price: 2_500,
                source: SourceKind::Wildberries,
                at: day(2, 10),
                since: day(1, 10),
            })
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn price_is_told_against_the_past_low() {
        let low = PastLow {
            price: 2_000,
            source: SourceKind::Wildberries,
            at: day(3, 10),
            since: day(1, 10),
        };

        assert_eq!(low.says(1_900), "Самая низкая цена с 01.11");
        assert_eq!(low.says(2_000), "Самая низкая цена с 01.11");
        assert_eq!(
            low.says(2_100),
            "Почти самая низкая с 01.11: было Wildberries 2\u{a0}000\u{a0}₽ (03.11)"
        );
        assert_eq!(
            low.says(2_500),
            "С 01.11 бывало дешевле: Wildberries 2\u{a0}000\u{a0}₽ (03.11)"
        );
    }

    #[test]
    fn what_is_older_than_a_month_is_forgotten() {
        let (memory, dir) = memory("old");
        memory.start(KEY, 1_000, SourceKind::Ozon, day(1, 10) - Duration::days(40));
        memory.start(KEY, 3_000, SourceKind::Ozon, day(1, 10));

        let low = memory.note(&sony(), Category::Any, SourceKind::Ozon, 2_900, day(2, 10));

        assert_eq!(low.map(|low| low.price), Some(3_000));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn searches_within_an_hour_make_one_point_with_the_lower_price() {
        let mut points = Vec::new();
        Points::add(&mut points, point(3_000, SourceKind::Ozon, day(1, 10)));
        Points::add(
            &mut points,
            point(2_500, SourceKind::Dns, day(1, 10) + Duration::minutes(20)),
        );
        Points::add(
            &mut points,
            point(2_700, SourceKind::Ozon, day(1, 10) + Duration::minutes(40)),
        );

        assert_eq!(points.len(), 1);
        assert_eq!((points[0].price, points[0].source), (2_500, SourceKind::Dns));
    }

    #[test]
    fn request_with_conditions_is_another_question() {
        let (memory, dir) = memory("conditions");
        memory.start(KEY, 1_000, SourceKind::Ozon, day(1, 10));

        let limited = SearchRequest::parse("наушники sony до 5000");

        assert_eq!(
            memory.note(&limited, Category::Any, SourceKind::Ozon, 2_000, day(2, 10)),
            None
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn history_keeps_the_query_as_it_was_written() {
        let (memory, dir) = memory("written");
        let request = SearchRequest::parse("наушники sony wh-ch520");

        memory.note(&request, Category::Any, SourceKind::Ozon, 3_000, day(1, 10));

        assert_eq!(memory.recent(10)[0].query, "наушники sony wh-ch520");
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn recent_queries_are_the_newest_once_each_with_their_lowest_price() {
        let (memory, dir) = memory("recent");
        memory.start(KEY, 3_000, SourceKind::Ozon, day(1, 10));
        memory.start(KEY, 2_800, SourceKind::Dns, day(3, 10));
        memory.start("Electronics: sony наушники", 2_900, SourceKind::Ozon, day(2, 10));
        memory.start("Any: чайник", 1_500, SourceKind::Ozon, day(4, 10));

        let recent = memory.recent(10);

        let lines: Vec<(&str, u64, u64)> = recent
            .iter()
            .map(|searched| (searched.query.as_str(), searched.price, searched.lowest))
            .collect();
        assert_eq!(lines, [("чайник", 1_500, 1_500), ("sony наушники", 2_800, 2_800)]);
        assert_eq!(memory.recent(1).len(), 1);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn another_category_is_another_question() {
        let (memory, dir) = memory("category");
        memory.start(KEY, 500, SourceKind::Ozon, day(1, 10));

        let low = memory.note(&sony(), Category::Electronics, SourceKind::Ozon, 2_000, day(2, 10));

        assert_eq!(low, None);
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn memory_survives_a_restart() {
        let (memory, dir) = memory("restart");
        memory.note(&sony(), Category::Any, SourceKind::Ozon, 3_000, day(1, 10));
        memory.note(&sony(), Category::Any, SourceKind::Ozon, 2_900, day(2, 10));

        let reloaded = PriceMemory::new(Storage::new(dir.clone()));

        assert!(
            reloaded
                .note(&sony(), Category::Any, SourceKind::Ozon, 2_800, day(3, 10))
                .is_some()
        );
        std::fs::remove_dir_all(dir).ok();
    }
}
