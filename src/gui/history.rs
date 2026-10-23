use chrono::{DateTime, Local, NaiveDateTime};
use eframe::egui::{self, Color32};
use egui_plot::{Corner, GridMark, HoverPosition, Legend, Line, Plot};

use super::GOOD;
use crate::hunt::{Lowest, MarketPoint, MarketPrice};

const MARKET: Color32 = Color32::from_rgb(0x4a, 0x90, 0xd9);
const SECONDS_IN_DAY: f64 = 86_400.0;
/// Шаги делений по оси времени в сутках: от трёх часов до недели.
const TIME_STEPS: [f64; 6] = [0.125, 0.25, 0.5, 1.0, 2.0, 7.0];
/// Столько подписей дат помещается под графиком в карточке слежки.
const TIME_MARKS: f64 = 6.0;

/// Строка «История рынка …» в карточке слежки; раскрывается в график рынка и самой низкой цены.
pub(super) fn show(ui: &mut egui::Ui, watch_id: u64, points: &[MarketPoint]) {
    let Some(title) = title(points) else {
        return;
    };
    egui::CollapsingHeader::new(title)
        .id_salt(("market-history", watch_id))
        .show(ui, |ui| chart(ui, watch_id, points));
}

/// «История рынка с 18.10: 4 352 ₽ → 4 100 ₽ (−6%), самая низкая 3 790 ₽ (12.10)». `None` — точка пока одна:
/// рынок последней проверки и так виден в строке состояния.
fn title(points: &[MarketPoint]) -> Option<String> {
    let [first, .., last] = points else {
        return None;
    };
    let lowest = Lowest::among(points)?;
    Some(format!(
        "История рынка с {}: {} → {} ({}), самая низкая {} ({})",
        first.at.format("%d.%m"),
        first.market,
        last.market,
        change(first.market.amount(), last.market.amount()),
        lowest.price,
        lowest.at.format("%d.%m")
    ))
}

fn change(from: u64, to: u64) -> String {
    let percent = to.abs_diff(from) * 100 / from.max(1);
    if percent == 0 {
        "без изменений".into()
    } else if to < from {
        format!("−{percent}%")
    } else {
        format!("+{percent}%")
    }
}

fn chart(ui: &mut egui::Ui, watch_id: u64, points: &[MarketPoint]) {
    let Some(sample) = points.last().map(|point| point.market) else {
        return;
    };
    let line = |price: fn(&MarketPoint) -> MarketPrice| -> Vec<[f64; 2]> {
        points
            .iter()
            .map(|point| [day(point.at), plotted(price(point))])
            .collect()
    };
    let market = line(|point| point.market);
    let lowest = line(|point| point.lowest);
    Plot::new(("market-chart", watch_id))
        .height(170.0)
        .allow_zoom(false)
        .allow_drag(false)
        .allow_scroll(false)
        .allow_boxed_zoom(false)
        .allow_double_click_reset(false)
        .legend(Legend::default().position(Corner::LeftBottom))
        .x_grid_spacer(|input| time_marks(input.bounds))
        .x_axis_formatter(|mark, _| time_label(mark.value))
        .y_axis_formatter(move |mark, _| price_label(sample, mark.value))
        .label_formatter(move |hover| match hover {
            HoverPosition::NearDataPoint {
                plot_name, position, ..
            } => Some(format!(
                "{plot_name}: {}\n{}",
                price_label(sample, position.y),
                moment(position.x)
                    .map(|at| at.format("%d.%m %H:%M").to_string())
                    .unwrap_or_default()
            )),
            HoverPosition::Elsewhere { .. } => None,
        })
        .show(ui, |plot| {
            plot.line(Line::new("рынок", market).color(MARKET).width(2.0));
            plot.line(Line::new("самая низкая", lowest).color(GOOD).width(2.0));
        });
}

/// Деления по оси времени: не больше шести, ровно в полночь или кратно трём часам.
fn time_marks((from, to): (f64, f64)) -> Vec<GridMark> {
    if !(from.is_finite() && to.is_finite()) {
        return Vec::new();
    }
    let step = TIME_STEPS
        .into_iter()
        .find(|&step| step * TIME_MARKS >= to - from)
        .unwrap_or(TIME_STEPS[TIME_STEPS.len() - 1]);
    let mut marks = Vec::new();
    let mut value = (from / step).ceil() * step;
    while value <= to {
        marks.push(GridMark { value, step_size: step });
        value += step;
    }
    marks
}

/// Полночь подписывается датой, остальные деления — временем.
fn time_label(day: f64) -> String {
    let format = if day.fract() == 0.0 { "%d.%m" } else { "%H:%M" };
    moment(day).map(|at| at.format(format).to_string()).unwrap_or_default()
}

/// Цены и время здесь намного меньше 2⁵³ — переводы между f64 и целыми точны.
#[allow(clippy::cast_precision_loss)]
fn plotted(price: MarketPrice) -> f64 {
    price.amount() as f64
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn price_label(sample: MarketPrice, value: f64) -> String {
    if value < 0.0 {
        return String::new();
    }
    sample.with_amount(value.round() as u64).to_string()
}

/// Ось времени — в сутках по местному времени: целое число — полночь, и даты встают ровно на деления.
#[allow(clippy::cast_precision_loss)]
fn day(at: DateTime<Local>) -> f64 {
    at.naive_local().and_utc().timestamp() as f64 / SECONDS_IN_DAY
}

#[allow(clippy::cast_possible_truncation)]
fn moment(day: f64) -> Option<NaiveDateTime> {
    DateTime::from_timestamp((day * SECONDS_IN_DAY).round() as i64, 0).map(|utc| utc.naive_utc())
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::quantity::{Unit, UnitPrice};

    fn point(day: u32, market: u64, lowest: u64) -> MarketPoint {
        MarketPoint {
            at: Local.with_ymd_and_hms(2026, 10, day, 12, 0, 0).unwrap(),
            market: MarketPrice::Whole(market),
            lowest: MarketPrice::Whole(lowest),
            comparable: 40,
        }
    }

    #[test]
    fn title_shows_market_change_and_the_lowest_price_with_dates() {
        let points = [
            point(18, 4_352, 3_900),
            point(19, 4_300, 3_790),
            point(20, 4_100, 3_850),
        ];
        assert_eq!(
            title(&points).unwrap(),
            "История рынка с 18.10: 4\u{a0}352\u{a0}₽ → 4\u{a0}100\u{a0}₽ (−5%), самая низкая 3\u{a0}790\u{a0}₽ (19.10)"
        );
        assert!(title(&points[..1]).is_none(), "по одной точке истории нет");
    }

    #[test]
    fn change_is_signed_and_small_moves_are_no_change() {
        assert_eq!(change(4_000, 4_400), "+10%");
        assert_eq!(change(4_000, 3_000), "−25%");
        assert_eq!(change(4_000, 4_030), "без изменений");
    }

    #[test]
    fn time_axis_has_at_most_six_marks_on_whole_days_or_hours() {
        let month = time_marks((20_000.3, 20_030.2));
        assert_eq!(
            month.iter().map(|mark| mark.value).collect::<Vec<_>>(),
            [20_006.0, 20_013.0, 20_020.0, 20_027.0]
        );

        let afternoon = time_marks((20_000.5, 20_000.8));
        assert_eq!(
            afternoon.iter().map(|mark| time_label(mark.value)).collect::<Vec<_>>(),
            ["12:00", "15:00", "18:00"]
        );
    }

    #[test]
    fn axis_labels_are_dates_and_prices_in_the_compared_units() {
        let noon = point(20, 4_100, 3_850).at;
        assert_eq!(time_label(day(noon).floor()), "20.10");
        assert_eq!(moment(day(noon)), Some(noon.naive_local()));

        let per_kilogram = MarketPrice::PerUnit(UnitPrice {
            kopecks: 31_000,
            unit: Unit::Kilogram,
        });
        assert_eq!(price_label(per_kilogram, 25_000.0), "250\u{a0}₽/кг");
        assert_eq!(price_label(MarketPrice::Whole(4_100), 3_999.6), "4\u{a0}000\u{a0}₽");
    }
}
