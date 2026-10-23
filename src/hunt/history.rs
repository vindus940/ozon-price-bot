use std::collections::HashMap;

use chrono::{DateTime, Duration, Local, Timelike};
use serde::{Deserialize, Serialize};

use crate::hunt::market::MarketPrice;
use crate::quantity::Unit;

pub const FILE: &str = "history.json";

/// Сколько дней помнится рынок слежки: столько показывает график и столько охватывает «самая низкая цена».
const KEPT_DAYS: i64 = 30;

/// Итог одной проверки, у которой получилось посчитать рынок.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MarketPoint {
    pub at: DateTime<Local>,
    pub market: MarketPrice,
    /// Самая низкая цена похожих, кроме подозрительно дешёвых.
    pub lowest: MarketPrice,
    pub comparable: usize,
}

/// Самая низкая цена за прошлые проверки — с ней сравнивается новая находка.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Lowest {
    pub price: MarketPrice,
    pub at: DateTime<Local>,
    /// С какой проверки есть история: слежка может быть моложе 30 дней.
    pub since: DateTime<Local>,
}

impl Lowest {
    /// Самая низкая среди точек, уже приведённых к одним единицам.
    pub fn among(points: &[MarketPoint]) -> Option<Self> {
        let since = points.first()?.at;
        let lowest = points.iter().min_by_key(|point| point.lowest.amount())?;
        Some(Self {
            price: lowest.lowest,
            at: lowest.at,
            since,
        })
    }

    /// Не дороже ли `price` самой низкой цены. `None` — цены за разное, их не сравнить.
    pub fn beaten_by(&self, price: MarketPrice) -> Option<bool> {
        (price.unit() == self.price.unit()).then(|| price.amount() <= self.price.amount())
    }
}

/// История рынка по слежкам: точка в час за последние 30 дней. Чаще не нужно — график за месяц, а файл
/// переписывается после каждой проверки: при проверке раз в 5 минут точка на каждую раздула бы его в 12 раз.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct History {
    by_watch: HashMap<u64, Vec<MarketPoint>>,
}

impl History {
    /// Проверка в тот же час, что и прошлая, заменяет её точку: рынок и число похожих — последние,
    /// а самая низкая цена — самая низкая за этот час.
    pub fn record(&mut self, watch_id: u64, point: MarketPoint) {
        let points = self.by_watch.entry(watch_id).or_default();
        match points.last_mut() {
            Some(last) if same_hour(last.at, point.at) => {
                *last = MarketPoint {
                    lowest: cheaper(last.lowest, point.lowest),
                    ..point
                };
            }
            _ => points.push(point),
        }
        let oldest = point.at - Duration::days(KEPT_DAYS);
        points.retain(|kept| kept.at > oldest);
    }

    /// Точки для графика — в тех единицах, что и последняя проверка: если расходник начали сравнивать
    /// в литрах вместо килограммов, старые цены за килограмм на одном графике с ними не покажешь.
    pub fn points(&self, watch_id: u64) -> Vec<MarketPoint> {
        let Some(last) = self.by_watch.get(&watch_id).and_then(|points| points.last()) else {
            return Vec::new();
        };
        self.in_unit(watch_id, last.market.unit()).collect()
    }

    /// Самая низкая цена за 30 дней до `now` среди проверок в тех же единицах.
    pub fn lowest(&self, watch_id: u64, now: DateTime<Local>, unit: Option<Unit>) -> Option<Lowest> {
        let oldest = now - Duration::days(KEPT_DAYS);
        let recent: Vec<MarketPoint> = self.in_unit(watch_id, unit).filter(|point| point.at > oldest).collect();
        Lowest::among(&recent)
    }

    pub fn forget(&mut self, watch_id: u64) {
        self.by_watch.remove(&watch_id);
    }

    fn in_unit(&self, watch_id: u64, unit: Option<Unit>) -> impl Iterator<Item = MarketPoint> + '_ {
        self.by_watch
            .get(&watch_id)
            .into_iter()
            .flatten()
            .filter(move |point| point.market.unit() == unit)
            .copied()
    }
}

fn same_hour(a: DateTime<Local>, b: DateTime<Local>) -> bool {
    (a.date_naive(), a.hour()) == (b.date_naive(), b.hour())
}

fn cheaper(old: MarketPrice, new: MarketPrice) -> MarketPrice {
    if old.unit() == new.unit() && old.amount() < new.amount() {
        old
    } else {
        new
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::quantity::UnitPrice;

    fn at(day: u32, hour: u32, minute: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, day, hour, minute, 0).unwrap()
    }

    fn point(at: DateTime<Local>, market: u64, lowest: u64) -> MarketPoint {
        MarketPoint {
            at,
            market: MarketPrice::Whole(market),
            lowest: MarketPrice::Whole(lowest),
            comparable: 40,
        }
    }

    fn per_kilogram(at: DateTime<Local>, rubles: u64) -> MarketPoint {
        let price = MarketPrice::PerUnit(UnitPrice {
            kopecks: rubles * 100,
            unit: Unit::Kilogram,
        });
        MarketPoint {
            at,
            market: price,
            lowest: price,
            comparable: 12,
        }
    }

    #[test]
    fn checks_within_one_hour_make_one_point_with_the_lowest_of_the_hour() {
        let mut history = History::default();
        history.record(1, point(at(20, 12, 0), 4_400, 3_900));
        history.record(1, point(at(20, 12, 15), 4_350, 3_700));
        history.record(1, point(at(20, 12, 45), 4_300, 3_800));
        history.record(1, point(at(20, 13, 0), 4_200, 3_850));

        assert_eq!(
            history.points(1),
            [point(at(20, 12, 45), 4_300, 3_700), point(at(20, 13, 0), 4_200, 3_850)]
        );
        assert!(history.points(2).is_empty(), "у другой слежки своя история");
    }

    #[test]
    fn only_the_last_30_days_are_kept() {
        let mut history = History::default();
        history.record(1, point(at(1, 12, 0), 4_400, 3_000));
        history.record(1, point(at(2, 12, 0), 4_400, 3_900));
        history.record(1, point(at(31, 13, 0), 4_300, 3_800));

        assert_eq!(
            history.points(1).iter().map(|point| point.at).collect::<Vec<_>>(),
            [at(2, 12, 0), at(31, 13, 0)]
        );
    }

    #[test]
    fn lowest_names_its_date_and_since_when_history_goes() {
        let mut history = History::default();
        history.record(1, point(at(10, 9, 0), 4_400, 3_900));
        history.record(1, point(at(12, 9, 0), 4_300, 3_600));
        history.record(1, point(at(15, 9, 0), 4_200, 3_800));

        let lowest = history.lowest(1, at(20, 12, 0), None).unwrap();

        assert_eq!(lowest.price, MarketPrice::Whole(3_600));
        assert_eq!((lowest.at, lowest.since), (at(12, 9, 0), at(10, 9, 0)));
        let month_later = history
            .lowest(1, at(11, 12, 0) + Duration::days(KEPT_DAYS), None)
            .unwrap();
        assert_eq!(
            (month_later.price, month_later.since),
            (MarketPrice::Whole(3_600), at(12, 9, 0)),
            "проверка 10-го уже старше 30 дней"
        );
        assert!(
            history
                .lowest(1, at(15, 9, 0) + Duration::days(KEPT_DAYS), None)
                .is_none(),
            "всё старше 30 дней — истории нет"
        );
        assert!(history.lowest(2, at(20, 12, 0), None).is_none());
    }

    #[test]
    fn deal_beats_the_lowest_only_at_its_price_or_cheaper_and_in_the_same_units() {
        let lowest = Lowest::among(&[point(at(12, 9, 0), 4_300, 3_600)]).unwrap();

        assert_eq!(lowest.beaten_by(MarketPrice::Whole(3_600)), Some(true));
        assert_eq!(lowest.beaten_by(MarketPrice::Whole(3_601)), Some(false));
        assert_eq!(lowest.beaten_by(per_kilogram(at(12, 9, 0), 300).lowest), None);
    }

    #[test]
    fn prices_in_other_units_are_not_mixed() {
        let mut history = History::default();
        history.record(1, per_kilogram(at(10, 9, 0), 250));
        history.record(1, point(at(11, 9, 0), 4_300, 3_600));
        history.record(1, per_kilogram(at(12, 9, 0), 310));

        assert_eq!(
            history
                .points(1)
                .iter()
                .map(|point| point.lowest.amount())
                .collect::<Vec<_>>(),
            [25_000, 31_000]
        );
        assert_eq!(
            history.lowest(1, at(20, 12, 0), None).map(|lowest| lowest.price),
            Some(MarketPrice::Whole(3_600))
        );
    }

    #[test]
    fn history_survives_saving() {
        let mut history = History::default();
        history.record(1, point(at(10, 9, 0), 4_400, 3_900));
        history.record(1, per_kilogram(at(11, 9, 0), 250));

        let saved = serde_json::to_string(&history).unwrap();
        let read: History = serde_json::from_str(&saved).unwrap();

        assert_eq!(read.by_watch, history.by_watch);
    }
}
