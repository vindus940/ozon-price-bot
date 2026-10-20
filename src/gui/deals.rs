use eframe::egui::{self, RichText};

use super::{BAD, GOOD};
use crate::format::{rub, signed_rub, tile_marks};
use crate::hunt::{Deal, Hunter, Miss};

/// Что показать после «мимо»: находка скрыта, а эти слова можно добавить слежке в стоп-слова.
pub(super) struct DismissNotice {
    watch_id: u64,
    watch_name: String,
    words: Vec<String>,
    added: Vec<String>,
}

pub(super) fn show(
    ui: &mut egui::Ui,
    hunter: &Hunter,
    filter: &mut Option<u64>,
    confirm_clear: &mut bool,
    notice: &mut Option<DismissNotice>,
) {
    let watches = hunter.watches();
    let deals = hunter.deals();
    if filter.is_some_and(|id| !watches.iter().any(|watch| watch.id == id)) {
        *filter = None;
    }

    ui.horizontal(|ui| {
        ui.heading("Находки");
        let selected = filter
            .and_then(|id| watches.iter().find(|watch| watch.id == id))
            .map_or("Все слежки".to_string(), |watch| watch.name.clone());
        egui::ComboBox::from_id_salt("deals-filter")
            .selected_text(selected)
            .width(220.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(filter, None, "Все слежки");
                for watch in &watches {
                    ui.selectable_value(filter, Some(watch.id), &watch.name);
                }
            });
        if *confirm_clear {
            ui.colored_label(BAD, "Удалить все находки из списка?");
            if ui.button("Да").clicked() {
                hunter.clear_deals();
                *confirm_clear = false;
            }
            if ui.button("Нет").clicked() {
                *confirm_clear = false;
            }
        } else if ui.button("Очистить список").clicked() {
            *confirm_clear = true;
        }
    });
    ui.weak(
        "Всё, что нашли слежки (о том же приходят уведомления в Telegram). Свежие — сверху, ссылка открывает \
         предложение в браузере. «Мимо» убирает находку насовсем: слежка её больше не пришлёт.",
    );
    ui.add_space(8.0);
    notice_panel(ui, hunter, notice);

    let visible: Vec<&Deal> = deals
        .iter()
        .filter(|deal| filter.is_none_or(|id| deal.watch_id == id))
        .collect();
    if visible.is_empty() {
        ui.add_space(30.0);
        ui.label("Находок пока нет. Они появятся, когда слежка найдёт предложение выгоднее рынка.");
        return;
    }

    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        for deal in visible {
            let mut miss = None;
            card(ui, deal, |ui| miss = miss_menu(ui));
            if let Some(miss) = miss {
                *notice = Some(DismissNotice {
                    watch_id: deal.watch_id,
                    watch_name: deal.watch_name.clone(),
                    words: hunter.dismiss(deal.watch_id, &deal.offer.url, miss),
                    added: Vec::new(),
                });
            }
            ui.add_space(6.0);
        }
    });
}

/// `actions` дорисовываются внизу карточки: во вкладке «Находки» это «Мимо», в пробной проверке — ничего.
pub(super) fn card(ui: &mut egui::Ui, deal: &Deal, actions: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(rub(deal.offer.price)).strong().size(18.0));
            if let Some(per_unit) = deal.per_unit {
                ui.label(per_unit.price.to_string());
            }
            ui.colored_label(GOOD, format!("−{}% к рынку", deal.discount_percent));
            let market = deal
                .per_unit
                .map_or_else(|| rub(deal.market_price), |per_unit| per_unit.market.to_string());
            ui.weak(format!("рынок {market} по {} похожим", deal.market_sample));
            if let Some(profit) = deal.resale_profit {
                ui.colored_label(GOOD, format!("выгода ≈ {}", signed_rub(profit)));
            }
            if let Some(km) = deal.offer.distance_km {
                ui.label(format!("{km:.1} км от дома"));
            }
        });
        let place: Vec<&str> = [deal.offer.place.as_deref(), deal.offer.posted.as_deref()]
            .into_iter()
            .flatten()
            .collect();
        if !place.is_empty() {
            ui.label(place.join(" · "));
        }
        if deal.suspicious {
            ui.colored_label(
                BAD,
                "Дешевле половины рынка — проверьте продавца и состояние, это частый признак обмана",
            );
        }
        ui.hyperlink_to(&deal.offer.name, &deal.offer.url);

        let mut details = vec![deal.watch_name.clone(), deal.found_at.format("%d.%m %H:%M").to_string()];
        if let Some(rating) = deal.offer.rating {
            details.push(format!("★ {rating}"));
        }
        if let Some(reviews) = deal.offer.reviews {
            details.push(format!("отзывов {reviews}"));
        }
        let marks = tile_marks(&deal.offer);
        if !marks.is_empty() {
            details.push(marks);
        }
        ui.weak(details.join(" · "));
        actions(ui);
    });
}

fn miss_menu(ui: &mut egui::Ui) -> Option<Miss> {
    let mut chosen = None;
    ui.menu_button("Мимо", |ui| {
        for miss in Miss::ALL {
            if ui.button(capitalized(miss.title())).clicked() {
                chosen = Some(miss);
                ui.close();
            }
        }
    });
    chosen
}

fn notice_panel(ui: &mut egui::Ui, hunter: &Hunter, notice: &mut Option<DismissNotice>) {
    let Some(current) = notice.as_mut() else {
        return;
    };
    let mut done = false;
    egui::Frame::group(ui.style()).inner_margin(8.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(format!(
            "Находка скрыта — слежка «{}» её больше не пришлёт.",
            current.watch_name
        ));
        let left: Vec<String> = current
            .words
            .iter()
            .filter(|word| !current.added.contains(word))
            .cloned()
            .collect();
        if !left.is_empty() {
            ui.horizontal_wrapped(|ui| {
                ui.label("Чтобы похожее не приходило, добавьте слежке стоп-слово:");
                for word in left {
                    if ui.button(&word).clicked() {
                        hunter.add_stop_word(current.watch_id, &word);
                        current.added.push(word);
                    }
                }
            });
        }
        if !current.added.is_empty() {
            ui.weak(format!("Добавлено в стоп-слова: {}", current.added.join(", ")));
        }
        done = ui.button("Готово").clicked();
    });
    if done {
        *notice = None;
    }
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_items_start_with_a_capital_letter() {
        assert_eq!(capitalized("б/у или брак"), "Б/у или брак");
        assert_eq!(capitalized(""), "");
    }
}
