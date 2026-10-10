use eframe::egui::{self, RichText};

use super::{BAD, GOOD};
use crate::format::{rub, signed_rub};
use crate::hunt::{Deal, Hunter};

pub(super) fn show(ui: &mut egui::Ui, hunter: &Hunter, filter: &mut Option<u64>, confirm_clear: &mut bool) {
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
    ui.weak("Всё, что нашли слежки (о том же приходят уведомления в Telegram). Свежие — сверху, ссылка открывает предложение в браузере.");
    ui.add_space(8.0);

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
            card(ui, deal);
            ui.add_space(6.0);
        }
    });
}

fn card(ui: &mut egui::Ui, deal: &Deal) {
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
        ui.weak(details.join(" · "));
    });
}
