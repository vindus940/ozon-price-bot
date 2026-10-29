use eframe::egui;

use super::deals::card;
use super::{BAD, GOOD, WARN};
use crate::format::rub;
use crate::hunt::{Assessment, Sifted, TrialStatus};
use crate::marketplace::Product;

const EXAMPLES: usize = 3;

/// Итог пробной проверки под формой слежки.
pub(super) fn show(ui: &mut egui::Ui, status: &TrialStatus) {
    match status {
        TrialStatus::Queued => {
            ui.colored_label(
                GOOD,
                "Проверка встала в очередь — начнётся, как только закончится текущая…",
            );
        }
        TrialStatus::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.colored_label(GOOD, "Ищу прямо сейчас — обычно это меньше минуты…");
            });
        }
        TrialStatus::Failed(reason) => {
            ui.colored_label(BAD, format!("Проверка не удалась — {reason}."));
        }
        TrialStatus::Done(assessment) => done(ui, assessment),
        TrialStatus::Priced(product) => {
            ui.label(&product.name);
            ui.colored_label(GOOD, product_price(product));
            ui.weak("Это пробная проверка: ничего не сохранено и никуда не отправлено.");
        }
    }
}

fn product_price(product: &Product) -> String {
    match (product.price, product.price_without_card) {
        (None, _) => "Сейчас товара нет в продаже".to_string(),
        (Some(price), Some(without_card)) if without_card != price => {
            format!("Сейчас {} с Ozon Картой, {} без неё", rub(price), rub(without_card))
        }
        (Some(price), _) => format!("Сейчас {}", rub(price)),
    }
}

fn done(ui: &mut egui::Ui, assessment: &Assessment) {
    let (summary, has_market) = summary(assessment);
    ui.colored_label(if has_market { GOOD } else { WARN }, summary);
    let reasons = sifted_lines(&assessment.sifted);
    if !reasons.is_empty() {
        ui.label("Не стали выгодными:");
        for reason in reasons {
            ui.label(format!("• {reason}"));
        }
    }
    if let Some(hint) = hint(assessment) {
        ui.colored_label(WARN, hint);
    }
    if !assessment.suitable.is_empty() {
        ui.label("Примеры выгодных:");
        egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
            for deal in assessment.suitable.iter().take(EXAMPLES) {
                card(ui, deal, |_| {});
                ui.add_space(4.0);
            }
        });
    }
    ui.weak("Это пробная проверка: ничего не сохранено и никуда не отправлено.");
}

/// Главная строка итога и есть ли рынок: без рынка выгодных не бывает, и это стоит выделить.
fn summary(assessment: &Assessment) -> (String, bool) {
    let (scanned, comparable) = (assessment.scanned, assessment.comparable);
    let Some(market) = assessment.market else {
        return (
            format!(
                "Найдено {scanned}, похожих только {comparable} — рыночную цену не посчитать. \
                 Уточните запрос или смените категорию."
            ),
            false,
        );
    };
    let hidden = match assessment.hidden_suspicious {
        0 => String::new(),
        hidden => format!(" Подозрительно дешёвых скрыто: {hidden}."),
    };
    (
        format!(
            "Найдено {scanned}, похожих {comparable}, рынок {market}, выгодных сейчас {}.{hidden}",
            assessment.suitable.len()
        ),
        true,
    )
}

fn sifted_lines(sifted: &Sifted) -> Vec<String> {
    [
        (sifted.other_words, "нет слов запроса или есть стоп-слово"),
        (
            sifted.other_product,
            "аксессуары, другие версии модели, брак или подделки",
        ),
        (sifted.used, "б/у — включено «Только новые»"),
        (sifted.copies, "повторы одного объявления"),
        (sifted.no_unit_price, "без фасовки или в другой единице"),
        (
            sifted.filters,
            "похожие, но не прошли ваши фильтры: цена, рейтинг, отзывы, доставка или продавец",
        ),
        (sifted.beyond_radius, "дальше вашего радиуса"),
        (
            sifted.not_cheap_enough,
            "похожие, но недостаточно дёшевы для вашей цели",
        ),
    ]
    .into_iter()
    .filter(|&(count, _)| count > 0)
    .map(|(count, reason)| format!("{count} — {reason}"))
    .collect()
}

/// Подсказка, что поменять, когда рынок есть, а выгодных нет: смотрим, что отсеяло больше всего.
fn hint(assessment: &Assessment) -> Option<&'static str> {
    if assessment.market.is_none() || !assessment.suitable.is_empty() {
        return None;
    }
    let sifted = &assessment.sifted;
    if sifted.not_cheap_enough >= sifted.filters.max(sifted.beyond_radius) && sifted.not_cheap_enough > 0 {
        Some(
            "Похожие есть, но не настолько дешевле рынка. \
             Можно уменьшить «дешевле рынка хотя бы на» или «выгоду от».",
        )
    } else if sifted.filters > 0 {
        Some("Похожие есть, но их отсеивают ваши фильтры — проверьте цену, рейтинг и галочки доставки.")
    } else if sifted.beyond_radius > 0 {
        Some("Похожие есть, но дальше радиуса — можно его увеличить.")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hunt::MarketPrice;

    fn checked(sifted: Sifted, suitable: usize) -> Assessment {
        let deal = crate::hunt::Deal {
            watch_id: 1,
            watch_name: "Айфон".into(),
            found_at: chrono::Local::now(),
            offer: crate::offer::Offer::sample("Apple iPhone 15 128GB", 55_000),
            market_price: 72_000,
            market_sample: 8,
            discount_percent: 23,
            resale_profit: None,
            suspicious: false,
            per_unit: None,
            price_before: None,
        };
        Assessment {
            scanned: 50,
            comparable: 32,
            market: Some(MarketPrice::Whole(72_000)),
            lowest: Some(MarketPrice::Whole(55_000)),
            suitable: vec![deal; suitable],
            hidden_suspicious: 2,
            sifted,
        }
    }

    #[test]
    fn summary_names_counts_market_and_hidden() {
        let (text, has_market) = summary(&checked(Sifted::default(), 4));
        assert!(has_market);
        assert_eq!(
            text,
            "Найдено 50, похожих 32, рынок 72\u{a0}000\u{a0}₽, выгодных сейчас 4. Подозрительно дешёвых скрыто: 2."
        );

        let without_market = Assessment {
            market: None,
            comparable: 3,
            ..checked(Sifted::default(), 0)
        };
        let (text, has_market) = summary(&without_market);
        assert!(!has_market);
        assert!(text.contains("похожих только 3"));
    }

    #[test]
    fn only_real_reasons_are_listed() {
        let lines = sifted_lines(&Sifted {
            other_product: 9,
            filters: 3,
            ..Sifted::default()
        });
        assert_eq!(
            lines,
            [
                "9 — аксессуары, другие версии модели, брак или подделки",
                "3 — похожие, но не прошли ваши фильтры: цена, рейтинг, отзывы, доставка или продавец",
            ]
        );
        assert!(sifted_lines(&Sifted::default()).is_empty());
    }

    #[test]
    fn hint_points_at_what_cut_the_most() {
        let not_cheap = checked(
            Sifted {
                not_cheap_enough: 20,
                filters: 5,
                ..Sifted::default()
            },
            0,
        );
        assert!(hint(&not_cheap).unwrap().contains("дешевле рынка хотя бы на"));

        let filtered = checked(
            Sifted {
                not_cheap_enough: 2,
                filters: 12,
                ..Sifted::default()
            },
            0,
        );
        assert!(hint(&filtered).unwrap().contains("фильтры"));

        let far = checked(
            Sifted {
                beyond_radius: 7,
                ..Sifted::default()
            },
            0,
        );
        assert!(hint(&far).unwrap().contains("радиуса"));

        assert!(
            hint(&checked(Sifted::default(), 2)).is_none(),
            "выгодные есть — подсказывать нечего"
        );
    }
}
