use std::sync::{Arc, Mutex};

use eframe::egui::{self, RichText};

use super::{BAD, GOOD, WARN};
use crate::format::{rub, tile_marks};
use crate::hunt::{BestPrices, Category, PriceProgress, PricedOffer, failure_reason, hunt_best_prices};
use crate::marketplace::SourceKind;
use crate::search::{OfferSearch, SearchRequest};

const QUERY_HINT: &str = "Например: наушники sony wh-ch520 до 5000";

enum Search {
    Idle,
    Running {
        waiting: Vec<&'static str>,
        found: Option<BestPrices>,
    },
    Done(BestPrices),
    Failed(String),
}

/// Вкладка «Поиск цены»: где сейчас дешевле всего купить один товар.
pub(super) struct PriceSearchTab {
    search: OfferSearch,
    runtime: tokio::runtime::Handle,
    query: String,
    category: Category,
    sources: Vec<(SourceKind, bool)>,
    new_only: bool,
    state: Arc<Mutex<Search>>,
}

impl PriceSearchTab {
    pub(super) fn new(search: OfferSearch, runtime: tokio::runtime::Handle) -> Self {
        Self {
            search,
            runtime,
            query: String::new(),
            category: Category::Any,
            sources: SourceKind::ALL.map(|source| (source, true)).to_vec(),
            new_only: false,
            state: Arc::new(Mutex::new(Search::Idle)),
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("Поиск цены");
        ui.label("Где сейчас дешевле всего: один запрос сразу на всех площадках.");
        ui.add_space(6.0);
        let running = matches!(*self.state.lock().unwrap(), Search::Running { .. });
        self.controls(ui, running);
        ui.add_space(8.0);
        ui.separator();
        match &*self.state.lock().unwrap() {
            Search::Idle => {
                ui.weak("Пишите как боту: слова товара, «до 5000», «-чехол», «рейтинг от 4.5». Чем точнее модель, тем честнее сравнение.");
            }
            Search::Running { waiting, found } => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(waiting_line(waiting));
                });
                if let Some(prices) = found {
                    results(ui, prices);
                }
            }
            Search::Failed(reason) => {
                ui.colored_label(BAD, reason);
            }
            Search::Done(prices) => results(ui, prices),
        }
    }

    fn controls(&mut self, ui: &mut egui::Ui, running: bool) {
        let mut start = false;
        ui.horizontal(|ui| {
            let field = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text(QUERY_HINT)
                    .desired_width(ui.available_width() - 90.0),
            );
            start = field.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
            start |= ui.add_enabled(!running, egui::Button::new("Найти")).clicked();
        });
        ui.horizontal_wrapped(|ui| {
            for (source, chosen) in &mut self.sources {
                ui.checkbox(chosen, source.title());
            }
            ui.separator();
            ui.checkbox(&mut self.new_only, "Только новые");
            ui.separator();
            egui::ComboBox::from_id_salt("price_search_category")
                .selected_text(self.category.title())
                .show_ui(ui, |ui| {
                    for category in Category::ALL {
                        ui.selectable_value(&mut self.category, category, category.title());
                    }
                });
        });
        if start && !running {
            self.start();
        }
    }

    fn start(&mut self) {
        let request = SearchRequest::parse(&self.query);
        let chosen: Vec<SourceKind> = self
            .sources
            .iter()
            .filter(|(_, chosen)| *chosen)
            .map(|(source, _)| *source)
            .collect();
        if let Some(problem) = problem(&request, &chosen) {
            *self.state.lock().unwrap() = Search::Failed(problem.to_string());
            return;
        }
        let search = self.search.only(&chosen);
        *self.state.lock().unwrap() = Search::Running {
            waiting: search.source_names(&request),
            found: None,
        };
        let (state, category, new_only) = (Arc::clone(&self.state), self.category, self.new_only);
        self.runtime.spawn(async move {
            let report = |progress: PriceProgress| {
                *state.lock().unwrap() = Search::Running {
                    waiting: progress.waiting.iter().map(|source| source.title()).collect(),
                    found: Some(progress.prices),
                };
            };
            let finished = match hunt_best_prices(&search, &request, category, new_only, &report).await {
                Ok(prices) => Search::Done(prices),
                Err((source, error)) => Search::Failed(failure_reason(source, &error)),
            };
            *state.lock().unwrap() = finished;
        });
    }
}

fn waiting_line(waiting: &[&str]) -> String {
    if waiting.is_empty() {
        "Первые ответы есть, смотрю выдачу от дешёвых…".to_string()
    } else {
        format!("Жду: {}", waiting.join(", "))
    }
}

fn problem(request: &SearchRequest, chosen: &[SourceKind]) -> Option<&'static str> {
    if request.query.is_empty() {
        Some("Напишите, что ищете: в запросе нет названия товара")
    } else if chosen.is_empty() {
        Some("Отметьте хотя бы одну площадку")
    } else {
        None
    }
}

fn results(ui: &mut egui::Ui, prices: &BestPrices) {
    ui.label(summary(prices));
    for (source, reason) in &prices.failed {
        ui.colored_label(WARN, format!("Без {}: {reason}", source.genitive()));
    }
    ui.add_space(6.0);
    match prices.best() {
        Some(best) => headline(ui, "Лучшая цена в магазине", best),
        None => {
            ui.colored_label(WARN, "Надёжной цены в магазинах не нашлось — уточните запрос.");
        }
    }
    if let Some(ad) = prices.best_ad() {
        headline(ui, "На Авито — новое или б/у, смотрите объявление", ad);
    }
    if !prices.lowest.is_empty() {
        ui.add_space(4.0);
        ui.label(lowest_line(prices));
    }
    ui.add_space(6.0);
    egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
        for found in &prices.offers {
            row(ui, found);
        }
    });
}

fn summary(prices: &BestPrices) -> String {
    let market = prices
        .market
        .map_or_else(String::new, |market| format!(", рынок {}", rub(market.median)));
    format!(
        "Просмотрено {}, тот же товар — {}, другой отсеян — {}{market}",
        prices.scanned,
        prices.offers.len(),
        prices.other_product
    )
}

fn lowest_line(prices: &BestPrices) -> String {
    let line: Vec<String> = prices
        .lowest
        .iter()
        .map(|&(source, price)| format!("{} {}", source.title(), rub(price)))
        .collect();
    format!("По площадкам: {}", line.join(" · "))
}

fn headline(ui: &mut egui::Ui, title: &str, found: &PricedOffer) {
    egui::Frame::group(ui.style()).inner_margin(10.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            ui.label(RichText::new(title).strong());
            ui.colored_label(GOOD, RichText::new(rub(found.offer.price)).strong().size(20.0));
            ui.label(found.source.title());
        });
        ui.hyperlink_to(&found.offer.name, &found.offer.url);
        details(ui, found);
    });
}

fn row(ui: &mut egui::Ui, found: &PricedOffer) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(rub(found.offer.price)).strong());
        ui.label(found.source.title());
        if found.used {
            ui.colored_label(WARN, "б/у");
        }
        if found.suspicious {
            ui.colored_label(BAD, "дешевле половины рынка");
        }
        ui.hyperlink_to(&found.offer.name, &found.offer.url);
    });
    details(ui, found);
    ui.add_space(4.0);
}

fn details(ui: &mut egui::Ui, found: &PricedOffer) {
    let line = details_line(found);
    if !line.is_empty() {
        ui.weak(line);
    }
}

fn details_line(found: &PricedOffer) -> String {
    let offer = &found.offer;
    let rating = offer.rating.map(|rating| match offer.reviews {
        Some(reviews) => format!("★ {rating}, отзывов {reviews}"),
        None => format!("★ {rating}"),
    });
    let marks = tile_marks(offer);
    [rating, offer.place.clone(), (!marks.is_empty()).then_some(marks)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offer::Offer;

    fn prices() -> BestPrices {
        BestPrices {
            scanned: 120,
            other_product: 30,
            offers: Vec::new(),
            market: None,
            lowest: vec![(SourceKind::Wildberries, 3_900), (SourceKind::Ozon, 4_000)],
            failed: Vec::new(),
        }
    }

    #[test]
    fn empty_query_and_no_sources_are_explained_before_going_to_the_sites() {
        let all = [SourceKind::Ozon];
        assert!(problem(&SearchRequest::parse("до 5000"), &all).is_some());
        assert!(problem(&SearchRequest::parse("наушники"), &[]).is_some());
        assert!(problem(&SearchRequest::parse("наушники"), &all).is_none());
    }

    #[test]
    fn status_names_who_is_still_silent() {
        assert_eq!(waiting_line(&["Ozon", "ДНС"]), "Жду: Ozon, ДНС");
        assert!(waiting_line(&[]).contains("от дешёвых"));
    }

    #[test]
    fn summary_says_how_much_was_seen_and_thrown_away() {
        assert_eq!(
            summary(&prices()),
            "Просмотрено 120, тот же товар — 0, другой отсеян — 30"
        );
    }

    #[test]
    fn lowest_line_names_each_source() {
        assert_eq!(
            lowest_line(&prices()),
            "По площадкам: Wildberries 3\u{a0}900\u{a0}₽ · Ozon 4\u{a0}000\u{a0}₽"
        );
    }

    #[test]
    fn details_join_only_what_is_known() {
        let found = |offer| PricedOffer {
            source: SourceKind::Ozon,
            offer,
            used: false,
            suspicious: false,
        };
        assert_eq!(details_line(&found(Offer::sample("Наушники", 1_000))), "");
        let rated = Offer {
            rating: Some(4.8),
            reviews: Some(25),
            place: Some("Москва".into()),
            ..Offer::sample("Наушники", 1_000)
        };
        assert_eq!(details_line(&found(rated)), "★ 4.8, отзывов 25 · Москва");
    }
}
