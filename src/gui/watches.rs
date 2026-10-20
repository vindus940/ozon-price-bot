use chrono::Duration;
use eframe::egui::{self, Color32, RichText};

use super::watch_form::WatchForm;
use super::{BAD, GOOD, WARN};
use crate::format::rub;
use crate::hunt::{Goal, Hunter, Outcome, RunStatus, Watch};

pub(super) fn show(ui: &mut egui::Ui, hunter: &Hunter, form: &mut Option<WatchForm>, pending_delete: &mut Option<u64>) {
    ui.horizontal(|ui| {
        ui.heading("Слежки");
        if ui.button(RichText::new("+ Новая слежка").strong()).clicked() {
            *form = Some(WatchForm::new());
        }
    });
    ui.weak("Слежка сама ищет товар по расписанию и пишет в Telegram, когда находит предложение выгоднее рынка.");
    ui.add_space(8.0);

    let watches = hunter.watches();
    if watches.is_empty() {
        ui.add_space(30.0);
        ui.label("Слежек пока нет. Нажмите «Новая слежка», напишите, что ищете, и выберите цель — купить ниже рынка или перепродать.");
        return;
    }

    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        for watch in &watches {
            card(ui, hunter, watch, form, pending_delete);
            ui.add_space(6.0);
        }
    });
}

fn card(
    ui: &mut egui::Ui,
    hunter: &Hunter,
    watch: &Watch,
    form: &mut Option<WatchForm>,
    pending_delete: &mut Option<u64>,
) {
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let mut enabled = watch.enabled;
            if ui
                .checkbox(&mut enabled, "")
                .on_hover_text("Включена ли слежка")
                .changed()
            {
                hunter.set_enabled(watch.id, enabled);
            }
            ui.label(RichText::new(&watch.name).strong().size(16.0));
            ui.weak(format!(
                "{} · {} · {}",
                watch.source.title(),
                watch.category.title(),
                goal_summary(watch.goal)
            ));
        });
        ui.label(conditions(watch));

        if hunter.is_connected(watch.source) {
            let activity = if hunter.is_checking(watch.id) {
                Activity::Checking
            } else if hunter.is_queued(watch.id) {
                Activity::Queued
            } else {
                Activity::Waiting
            };
            let (text, color) = status_line(watch, activity, hunter.run_status(watch.id));
            ui.colored_label(color, text);
        } else {
            ui.colored_label(
                WARN,
                format!("{} пока не подключено — слежка ждёт подключения", watch.source.title()),
            );
        }

        ui.horizontal(|ui| {
            if ui.button("Проверить сейчас").clicked() {
                hunter.run_now(watch.id);
            }
            if ui.button("Изменить").clicked() {
                *form = Some(WatchForm::edit(watch));
            }
            if *pending_delete == Some(watch.id) {
                ui.colored_label(BAD, "Удалить слежку и память о её находках?");
                if ui.button("Да, удалить").clicked() {
                    hunter.delete_watch(watch.id);
                    *pending_delete = None;
                }
                if ui.button("Нет").clicked() {
                    *pending_delete = None;
                }
            } else if ui.button("Удалить").clicked() {
                *pending_delete = Some(watch.id);
            }
        });
    });
}

fn goal_summary(goal: Goal) -> String {
    match goal {
        Goal::BelowMarket { min_discount_percent } => format!("ниже рынка от {min_discount_percent}%"),
        Goal::Resale { min_profit, .. } => format!("перепродажа: выгода от {}", rub(min_profit)),
    }
}

fn conditions(watch: &Watch) -> String {
    let mut parts = vec![format!("ищу «{}»", watch.query)];
    if watch.link.is_some() {
        parts.push("по ссылке с фильтрами сайта".into());
    }
    if let Some(radius) = watch.radius_km {
        parts.push(format!("до {radius} км от дома"));
    }
    match (watch.price.min, watch.price.max) {
        (Some(min), Some(max)) => parts.push(format!("цена {}–{}", rub(min), rub(max))),
        (Some(min), None) => parts.push(format!("цена от {}", rub(min))),
        (None, Some(max)) => parts.push(format!("цена до {}", rub(max))),
        (None, None) => {}
    }
    if let Some(rating) = watch.min_rating {
        parts.push(format!("рейтинг от {rating}"));
    }
    if let Some(reviews) = watch.min_reviews {
        parts.push(format!("отзывов от {reviews}"));
    }
    parts.extend(watch.shop.labels().into_iter().map(String::from));
    if watch.new_only {
        parts.push("только новые".into());
    }
    if !watch.stop_words.is_empty() {
        parts.push(format!("без: {}", watch.stop_words.join(", ")));
    }
    parts.push(format!("каждые {} мин", watch.every_minutes));
    parts.join(" · ")
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Activity {
    Checking,
    Queued,
    Waiting,
}

/// Строка состояния под слежкой: что происходит сейчас, чем кончилась последняя проверка и когда следующая.
fn status_line(watch: &Watch, activity: Activity, last_run: Option<RunStatus>) -> (String, Color32) {
    let normal = Color32::GRAY;
    match activity {
        Activity::Checking => return ("Проверяю прямо сейчас…".into(), GOOD),
        Activity::Queued => {
            return (
                "Встала в очередь на проверку — начнётся, как только закончится текущая".into(),
                GOOD,
            );
        }
        Activity::Waiting => {}
    }
    let Some(run) = last_run else {
        return if watch.enabled {
            (
                "Ещё не проверялась — первая проверка начнётся в течение минуты".into(),
                normal,
            )
        } else {
            ("Выключена".into(), normal)
        };
    };

    let at = run.at.format("%H:%M");
    let next = if watch.enabled {
        let next = run.at + Duration::minutes(i64::from(watch.every_minutes));
        format!(" Следующая проверка в {}.", next.format("%H:%M"))
    } else {
        String::new()
    };
    match run.outcome {
        Outcome::Checked {
            scanned,
            comparable,
            market: Some(market),
            suitable,
            news,
            hidden_suspicious,
        } => {
            let hidden = if hidden_suspicious > 0 {
                format!(" Подозрительно дешёвых скрыто: {hidden_suspicious}.")
            } else {
                String::new()
            };
            (
                format!(
                    "{at}: найдено {scanned}, похожих {comparable}, рынок {market}, выгодных сейчас {suitable}, новых {news}.{hidden}{next}"
                ),
                normal,
            )
        }
        Outcome::Checked {
            scanned, comparable, ..
        } => (
            format!(
                "{at}: найдено {scanned}, похожих только {comparable} — рыночную цену не посчитать. Уточните запрос или смените категорию.{next}"
            ),
            WARN,
        ),
        Outcome::Failed(reason) => (format!("{at}: проверка не удалась — {reason}.{next}"), BAD),
    }
}

#[cfg(test)]
mod tests {
    use chrono::{Local, TimeZone};

    use super::*;
    use crate::hunt::MarketPrice;
    use crate::search::PriceRange;

    fn drill() -> Watch {
        Watch {
            name: "Шуруповёрт".into(),
            query: "шуруповерт makita".into(),
            radius_km: Some(5),
            price: PriceRange {
                min: None,
                max: Some(8_000),
            },
            min_rating: Some(4.5),
            stop_words: vec!["тушка".into(), "без акб".into()],
            every_minutes: 15,
            ..Watch::new_draft()
        }
    }

    fn switched_off() -> Watch {
        Watch {
            enabled: false,
            ..drill()
        }
    }

    fn at_noon(outcome: Outcome) -> RunStatus {
        let at = Local.with_ymd_and_hms(2026, 10, 3, 12, 0, 0).unwrap();
        RunStatus { at, outcome }
    }

    fn checked(market: Option<MarketPrice>) -> Outcome {
        Outcome::Checked {
            scanned: 80,
            comparable: 45,
            market,
            suitable: 9,
            news: 2,
            hidden_suspicious: 5,
        }
    }

    #[test]
    fn conditions_read_as_one_line() {
        assert_eq!(
            conditions(&drill()),
            "ищу «шуруповерт makita» · до 5 км от дома · цена до 8\u{a0}000\u{a0}₽ · рейтинг от 4.5 · \
             без: тушка, без акб · каждые 15 мин"
        );
        let by_link = Watch {
            link: Some("https://www.avito.ru/moskva/instrumenty?f=ASgBAgICAUT4vA2W0jQ".into()),
            ..drill()
        };
        assert!(conditions(&by_link).starts_with("ищу «шуруповерт makita» · по ссылке с фильтрами сайта · "));
    }

    #[test]
    fn what_happens_now_beats_the_last_result() {
        let last = Some(at_noon(checked(None)));
        assert_eq!(
            status_line(&drill(), Activity::Checking, last.clone()),
            ("Проверяю прямо сейчас…".to_string(), GOOD)
        );
        assert!(
            status_line(&drill(), Activity::Queued, last)
                .0
                .starts_with("Встала в очередь")
        );
    }

    #[test]
    fn never_checked_watch_says_when_it_starts_or_that_it_is_off() {
        assert!(
            status_line(&drill(), Activity::Waiting, None)
                .0
                .starts_with("Ещё не проверялась")
        );
        assert_eq!(status_line(&switched_off(), Activity::Waiting, None).0, "Выключена");
    }

    #[test]
    fn checked_watch_shows_market_hidden_fakes_and_next_check() {
        let last = Some(at_noon(checked(Some(MarketPrice::Whole(4_463)))));
        assert_eq!(
            status_line(&drill(), Activity::Waiting, last),
            (
                "12:00: найдено 80, похожих 45, рынок 4\u{a0}463\u{a0}₽, выгодных сейчас 9, новых 2. \
                 Подозрительно дешёвых скрыто: 5. Следующая проверка в 12:15."
                    .to_string(),
                Color32::GRAY
            )
        );
    }

    #[test]
    fn too_few_comparables_is_a_warning() {
        let (text, color) = status_line(&drill(), Activity::Waiting, Some(at_noon(checked(None))));
        assert!(text.contains("похожих только 45 — рыночную цену не посчитать"));
        assert_eq!(color, WARN);
    }

    #[test]
    fn failed_check_of_switched_off_watch_has_no_next_time() {
        let last = Some(at_noon(Outcome::Failed("Авито просит проверку".into())));
        assert_eq!(
            status_line(&switched_off(), Activity::Waiting, last),
            ("12:00: проверка не удалась — Авито просит проверку.".to_string(), BAD)
        );
    }
}
