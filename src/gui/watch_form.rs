use std::str::FromStr;

use eframe::egui::{self, RichText};

use super::{BAD, WARN};
use crate::hunt::{Category, Goal, Watch};
use crate::marketplace::SourceKind;
use crate::search::PriceRange;

pub(super) enum FormAction {
    Keep,
    Save,
    Cancel,
}

#[derive(Clone, Copy, PartialEq)]
enum GoalKind {
    BelowMarket,
    Resale,
}

/// Черновик слежки: числа хранятся текстом, пока вы их печатаете, и проверяются при сохранении.
pub(super) struct WatchForm {
    id: u64,
    enabled: bool,
    name: String,
    source: SourceKind,
    category: Category,
    query: String,
    price_min: String,
    price_max: String,
    stop_words: String,
    min_rating: String,
    min_reviews: String,
    radius_km: Option<u32>,
    goal: GoalKind,
    min_discount: u8,
    min_profit: u64,
    costs: u64,
    include_suspicious: bool,
    every_minutes: u32,
    pub(super) problems: Vec<String>,
}

impl WatchForm {
    pub(super) fn new() -> Self {
        Self::edit(&Watch::new_draft())
    }

    pub(super) fn edit(watch: &Watch) -> Self {
        let (goal, min_discount, min_profit, costs) = match watch.goal {
            Goal::BelowMarket { min_discount_percent } => (GoalKind::BelowMarket, min_discount_percent, 2_000, 500),
            Goal::Resale { min_profit, costs } => (GoalKind::Resale, 20, min_profit, costs),
        };
        let number = |value: Option<u64>| value.map(|v| v.to_string()).unwrap_or_default();
        Self {
            id: watch.id,
            enabled: watch.enabled,
            name: watch.name.clone(),
            source: watch.source,
            category: watch.category,
            query: watch.query.clone(),
            price_min: number(watch.price.min),
            price_max: number(watch.price.max),
            stop_words: watch.stop_words.join(", "),
            min_rating: watch.min_rating.map(|r| r.to_string()).unwrap_or_default(),
            min_reviews: number(watch.min_reviews),
            radius_km: watch.radius_km,
            goal,
            min_discount,
            min_profit,
            costs,
            include_suspicious: watch.include_suspicious,
            every_minutes: watch.every_minutes,
            problems: Vec::new(),
        }
    }

    pub(super) fn title(&self) -> String {
        if self.id == 0 {
            "Новая слежка".to_string()
        } else {
            format!("Слежка «{}»", self.name)
        }
    }

    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        home_is_set: bool,
        is_connected: impl Fn(SourceKind) -> bool,
    ) -> FormAction {
        egui::Grid::new("watch-form")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                self.what_and_where(ui, home_is_set, &is_connected);
                self.what_we_look_for(ui);
                self.filters(ui);
                self.goal_and_schedule(ui);
            });

        for problem in &self.problems {
            ui.colored_label(BAD, format!("• {problem}"));
        }
        ui.add_space(8.0);

        let mut action = FormAction::Keep;
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Сохранить").strong()).clicked() {
                action = FormAction::Save;
            }
            if ui.button("Отмена").clicked() {
                action = FormAction::Cancel;
            }
        });
        action
    }

    fn what_and_where(&mut self, ui: &mut egui::Ui, home_is_set: bool, is_connected: &impl Fn(SourceKind) -> bool) {
        ui.label("Название");
        field(ui, &mut self.name, "например: Айфон 15 подешевле");
        ui.end_row();

        ui.label("Где искать");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                for source in SourceKind::ALL {
                    ui.radio_value(&mut self.source, source, source.title());
                }
            });
            if !is_connected(self.source) {
                ui.colored_label(
                    WARN,
                    format!(
                        "{} пока не подключено: слежка начнёт работать после подключения",
                        self.source.title()
                    ),
                );
            }
        });
        ui.end_row();

        if self.source.has_locations() {
            ui.label("Рядом с домом");
            ui.vertical(|ui| self.radius_choice(ui, home_is_set));
            ui.end_row();
        }
    }

    fn radius_choice(&mut self, ui: &mut egui::Ui, home_is_set: bool) {
        let title = |radius: Option<u32>| radius.map_or("не важно".to_string(), |km| format!("до {km} км"));
        egui::ComboBox::from_id_salt("radius")
            .selected_text(title(self.radius_km))
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut self.radius_km, None, title(None));
                for &km in self.source.radii_km() {
                    ui.selectable_value(&mut self.radius_km, Some(km), title(Some(km)));
                }
            });
        ui.weak(format!(
            "Рыночная цена считается по объявлениям в {} км от дома, а присылаются только те, что в выбранном радиусе.",
            self.source.radii_km().iter().max().copied().unwrap_or_default()
        ));
        if self.radius_km.is_some() && !home_is_set {
            ui.colored_label(
                WARN,
                "Укажите координаты дома в «Настройках» — без них радиус не посчитать",
            );
        }
    }

    fn what_we_look_for(&mut self, ui: &mut egui::Ui) {
        ui.label("Что ищем");
        field(ui, &mut self.query, "например: iphone 15 128");
        ui.end_row();

        ui.label("Категория");
        ui.vertical(|ui| {
            egui::ComboBox::from_id_salt("category")
                .selected_text(self.category.title())
                .width(260.0)
                .show_ui(ui, |ui| {
                    for category in Category::ALL {
                        ui.selectable_value(&mut self.category, category, category.title());
                    }
                });
            ui.weak(self.category.hint());
        });
        ui.end_row();
    }

    fn filters(&mut self, ui: &mut egui::Ui) {
        ui.label("Цена, ₽");
        ui.horizontal(|ui| {
            ui.label("от");
            short_field(ui, &mut self.price_min);
            ui.label("до");
            short_field(ui, &mut self.price_max);
            ui.weak("можно оставить пустым");
        });
        ui.end_row();

        ui.label("Стоп-слова");
        field(ui, &mut self.stop_words, "через запятую: восстановленный, витринный");
        ui.end_row();

        ui.label("Качество");
        ui.horizontal(|ui| {
            ui.label("рейтинг от");
            short_field(ui, &mut self.min_rating);
            ui.label("отзывов от");
            short_field(ui, &mut self.min_reviews);
            ui.weak("у Авито — рейтинг продавца; без рейтинга не пройдут");
        });
        ui.end_row();

        ui.label("Слишком дёшево");
        ui.vertical(|ui| {
            ui.checkbox(
                &mut self.include_suspicious,
                "Присылать и то, что дешевле половины рынка",
            );
            ui.weak(
                "Обычно это подделка или обман. Без галочки такие не присылаются, но их число видно в статусе слежки.",
            );
        });
        ui.end_row();
    }

    fn goal_and_schedule(&mut self, ui: &mut egui::Ui) {
        ui.label("Цель");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.goal, GoalKind::BelowMarket, "Купить ниже рынка");
                ui.radio_value(&mut self.goal, GoalKind::Resale, "Перепродать");
            });
            self.goal_details(ui);
        });
        ui.end_row();

        ui.label("Проверять");
        ui.horizontal(|ui| {
            ui.label("каждые");
            ui.add(
                egui::DragValue::new(&mut self.every_minutes)
                    .range(1..=1440)
                    .suffix(" мин"),
            );
            ui.weak(format!(
                "{} — не чаще раза в {} мин",
                self.source.title(),
                self.source.min_interval_minutes()
            ));
        });
        ui.end_row();
    }

    fn goal_details(&mut self, ui: &mut egui::Ui) {
        match self.goal {
            GoalKind::BelowMarket => {
                ui.horizontal(|ui| {
                    ui.label("дешевле рынка хотя бы на");
                    ui.add(egui::Slider::new(&mut self.min_discount, 5..=60).suffix("%"));
                });
                ui.weak(
                    "Рынок — медиана цен похожих предложений. Дешевле половины рынка помечается как подозрительное.",
                );
            }
            GoalKind::Resale => {
                ui.horizontal(|ui| {
                    ui.label("выгода от");
                    ui.add(
                        egui::DragValue::new(&mut self.min_profit)
                            .range(0..=1_000_000)
                            .speed(100)
                            .suffix(" ₽"),
                    );
                    ui.label("расходы");
                    ui.add(
                        egui::DragValue::new(&mut self.costs)
                            .range(0..=100_000)
                            .speed(50)
                            .suffix(" ₽"),
                    );
                });
                ui.weak("Выгода = рынок − 10% на торг − цена покупки − расходы (дорога, упаковка, продвижение).");
                if self.radius_km.is_none() {
                    ui.colored_label(
                        WARN,
                        "Для перепродажи выберите выше радиус «рядом с домом» — забирать придётся самому",
                    );
                }
            }
        }
    }

    /// Собирает слежку из полей. Ошибки чисел — здесь, остальные проверки делает сама слежка при сохранении.
    pub(super) fn to_watch(&self) -> Result<Watch, Vec<String>> {
        let mut problems = Vec::new();
        let price = PriceRange {
            min: optional(&self.price_min, "Цена от", &mut problems),
            max: optional(&self.price_max, "Цена до", &mut problems),
        };
        let min_rating = optional(&self.min_rating, "Рейтинг от", &mut problems);
        let min_reviews = optional(&self.min_reviews, "Отзывов от", &mut problems);
        if !problems.is_empty() {
            return Err(problems);
        }

        Ok(Watch {
            id: self.id,
            name: self.name.trim().to_string(),
            source: self.source,
            category: self.category,
            query: self.query.trim().to_string(),
            price,
            stop_words: self
                .stop_words
                .split(',')
                .map(str::trim)
                .filter(|word| !word.is_empty())
                .map(str::to_string)
                .collect(),
            min_rating,
            min_reviews,
            radius_km: self.radius_km.filter(|_| self.source.has_locations()),
            goal: match self.goal {
                GoalKind::BelowMarket => Goal::BelowMarket {
                    min_discount_percent: self.min_discount,
                },
                GoalKind::Resale => Goal::Resale {
                    min_profit: self.min_profit,
                    costs: self.costs,
                },
            },
            include_suspicious: self.include_suspicious,
            every_minutes: self.every_minutes,
            enabled: self.enabled,
        })
    }
}

fn field(ui: &mut egui::Ui, text: &mut String, hint: &str) {
    ui.add(egui::TextEdit::singleline(text).hint_text(hint).desired_width(380.0));
}

fn short_field(ui: &mut egui::Ui, text: &mut String) {
    ui.add(egui::TextEdit::singleline(text).desired_width(80.0));
}

/// Пустое поле — условия нет. Пробелы в «60 000» и запятая в «4,5» допустимы.
fn optional<T: FromStr>(text: &str, field: &str, problems: &mut Vec<String>) -> Option<T> {
    let cleaned = text.trim().replace([' ', '\u{a0}'], "").replace(',', ".");
    if cleaned.is_empty() {
        return None;
    }
    cleaned
        .parse()
        .map_err(|_| problems.push(format!("«{field}» — не число: {}", text.trim())))
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_read_with_spaces_and_commas() {
        let mut problems = Vec::new();
        assert_eq!(optional::<u64>("60 000", "Цена до", &mut problems), Some(60_000));
        assert_eq!(optional::<f32>("4,5", "Рейтинг от", &mut problems), Some(4.5));
        assert_eq!(optional::<u64>("  ", "Цена от", &mut problems), None);
        assert!(problems.is_empty());

        assert_eq!(optional::<u64>("шесть", "Цена до", &mut problems), None);
        assert_eq!(problems, ["«Цена до» — не число: шесть"]);
    }

    #[test]
    fn edited_watch_survives_the_form_unchanged() {
        let watch = Watch {
            id: 3,
            name: "Шуруповёрт".into(),
            source: SourceKind::Avito,
            category: Category::Tools,
            query: "шуруповерт makita".into(),
            price: PriceRange {
                min: Some(1_000),
                max: Some(8_000),
            },
            stop_words: vec!["тушка".into(), "без акб".into()],
            min_rating: Some(4.5),
            min_reviews: Some(10),
            radius_km: Some(5),
            goal: Goal::Resale {
                min_profit: 2_500,
                costs: 300,
            },
            include_suspicious: true,
            every_minutes: 20,
            enabled: false,
        };
        assert_eq!(WatchForm::edit(&watch).to_watch(), Ok(watch));
    }
}
