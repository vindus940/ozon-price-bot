use std::str::FromStr;

use eframe::egui::{self, RichText};

use super::{BAD, WARN, trial};
use crate::hunt::{Category, Goal, Hours, TrialStatus, Watch};
use crate::marketplace::SourceKind;
use crate::search::{PriceRange, ShopConditions};

/// Часы, которые форма предлагает, когда ставят галочку «Только в часы».
const DAYTIME: Hours = Hours { from: 8, to: 23 };

pub(super) enum FormAction {
    Keep,
    Try,
    Save,
    Cancel,
}

#[derive(Clone, Copy, PartialEq)]
enum GoalKind {
    BelowMarket,
    Resale,
    TargetPrice,
}

/// Цель слежки за товаром, пока вы её печатаете.
struct TargetDraft {
    price: String,
    without_card: bool,
}

/// Черновик слежки: числа хранятся текстом, пока вы их печатаете, и проверяются при сохранении.
pub(super) struct WatchForm {
    id: u64,
    enabled: bool,
    name: String,
    source: SourceKind,
    category: Category,
    query: String,
    link: String,
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
    target: TargetDraft,
    include_suspicious: bool,
    shop: ShopConditions,
    new_only: bool,
    every_minutes: u32,
    hours: Option<Hours>,
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
            Goal::TargetPrice { .. } => (GoalKind::TargetPrice, 20, 2_000, 500),
        };
        let number = |value: Option<u64>| value.map(|v| v.to_string()).unwrap_or_default();
        Self {
            id: watch.id,
            enabled: watch.enabled,
            name: watch.name.clone(),
            source: watch.source,
            category: watch.category,
            query: watch.query.clone(),
            link: watch.link.clone().unwrap_or_default(),
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
            target: TargetDraft {
                price: number(watch.target_price()),
                without_card: matches!(watch.goal, Goal::TargetPrice { without_card: true, .. }),
            },
            include_suspicious: watch.include_suspicious,
            shop: watch.shop,
            new_only: watch.new_only,
            every_minutes: watch.every_minutes,
            hours: watch.hours,
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
        trial: Option<&TrialStatus>,
    ) -> FormAction {
        egui::Grid::new("watch-form")
            .num_columns(2)
            .spacing([14.0, 10.0])
            .show(ui, |ui| {
                self.what_and_where(ui, home_is_set, &is_connected);
                self.goal(ui);
                if self.goal == GoalKind::TargetPrice {
                    self.product(ui);
                } else {
                    self.what_we_look_for(ui);
                    self.filters(ui);
                }
                self.schedule(ui);
            });

        for problem in &self.problems {
            ui.colored_label(BAD, format!("• {problem}"));
        }
        ui.add_space(8.0);

        let trying = matches!(trial, Some(TrialStatus::Queued | TrialStatus::Checking));
        let mut action = FormAction::Keep;
        ui.horizontal(|ui| {
            if ui.button(RichText::new("Сохранить").strong()).clicked() {
                action = FormAction::Save;
            }
            if ui
                .add_enabled(!trying, egui::Button::new("Проверить, не сохраняя"))
                .on_hover_text("Поискать сейчас и показать, что нашлось и почему отсеяно, — без записи и уведомлений")
                .clicked()
            {
                action = FormAction::Try;
            }
            if ui.button("Отмена").clicked() {
                action = FormAction::Cancel;
            }
        });
        if let Some(status) = trial {
            ui.separator();
            trial::show(ui, status);
        }
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
            if let Some(note) = self.source.note() {
                ui.weak(note);
            }
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

        if self.source.has_locations() && self.goal != GoalKind::TargetPrice {
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

        ui.label("Ссылка на выдачу");
        ui.vertical(|ui| {
            field(
                ui,
                &mut self.link,
                &format!("необязательно: https://{}/…", self.source.site()),
            );
            ui.weak(
                "Настройте фильтры на самом сайте в окне браузера программы (у Авито — «Состояние: новое») \
                 и вставьте адрес из строки браузера. Слова выше по-прежнему решают, что считать тем же товаром. \
                 Фильтры ссылки сужают и рынок — цену, рейтинг и доставку лучше задавать полями ниже.",
            );
        });
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

        if self.source.has_delivery() {
            ui.label("Доставка и продавец");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.shop.by_tomorrow, "Привезут не позже завтра");
                if self.source.has_marketplace_seller() {
                    ui.checkbox(&mut self.shop.only_ozon, "Только Ozon");
                }
                ui.weak("рынок всё равно считается по всем предложениям");
            });
            ui.end_row();
        }

        if self.source.has_used_goods() {
            ui.label("Состояние");
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.new_only, "Только новые — без б/у");
                ui.weak("по словам продавца: «б/у», «пользовался», «состояние 9/10»; рынок тоже только по новым");
            });
            ui.end_row();
        }

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

    fn goal(&mut self, ui: &mut egui::Ui) {
        ui.label("Цель");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.radio_value(&mut self.goal, GoalKind::BelowMarket, "Купить ниже рынка");
                ui.radio_value(&mut self.goal, GoalKind::Resale, "Перепродать");
                ui.radio_value(&mut self.goal, GoalKind::TargetPrice, "Дождаться цены товара");
            });
            self.goal_details(ui);
        });
        ui.end_row();
    }

    fn product(&mut self, ui: &mut egui::Ui) {
        ui.label("Ссылка на товар");
        ui.vertical(|ui| {
            field(ui, &mut self.link, &format!("https://{}/product/…", self.source.site()));
            ui.weak("Откройте страницу товара на сайте и скопируйте адрес из строки браузера.");
            if !self.source.has_product_pages() {
                ui.colored_label(WARN, "За ценой товара программа следит на Ozon и в ДНС");
            }
        });
        ui.end_row();

        ui.label("Цена, ₽");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.label("не дороже");
                short_field(ui, &mut self.target.price);
            });
            if self.source.has_card_price() {
                ui.checkbox(
                    &mut self.target.without_card,
                    "У меня нет Ozon Карты — сравнивать с ценой без неё",
                );
                ui.weak("Ozon показывает первой цену с Ozon Картой; в сообщении будут обе.");
            }
        });
        ui.end_row();
    }

    fn schedule(&mut self, ui: &mut egui::Ui) {
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

        ui.label("Часы работы");
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                let mut limited = self.hours.is_some();
                if ui.checkbox(&mut limited, "Только в часы").changed() {
                    self.hours = limited.then_some(DAYTIME);
                }
                if let Some(hours) = &mut self.hours {
                    ui.label("с");
                    ui.add(egui::DragValue::new(&mut hours.from).range(0..=23).suffix(":00"));
                    ui.label("до");
                    ui.add(egui::DragValue::new(&mut hours.to).range(0..=23).suffix(":00"));
                }
            });
            ui.weak(
                "Вне этих часов слежка не заходит на сайт и ничего не присылает; «Проверить сейчас» работает всегда. \
                 Можно и через полночь — с 22 до 7.",
            );
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
            GoalKind::TargetPrice => {
                ui.weak(
                    "Один конкретный товар: напишу, когда цена опустится до вашей, и коротко — если снова поднимется.",
                );
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
        let target_price = optional(&self.target.price, "Цена не дороже", &mut problems);
        if !problems.is_empty() {
            return Err(problems);
        }
        let product = self.goal == GoalKind::TargetPrice;

        Ok(Watch {
            id: self.id,
            name: self.name.trim().to_string(),
            source: self.source,
            category: self.category,
            query: self.query.trim().to_string(),
            link: Some(self.link.trim().to_string()).filter(|link| !link.is_empty()),
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
            radius_km: self.radius_km.filter(|_| self.source.has_locations() && !product),
            goal: match self.goal {
                GoalKind::BelowMarket => Goal::BelowMarket {
                    min_discount_percent: self.min_discount,
                },
                GoalKind::Resale => Goal::Resale {
                    min_profit: self.min_profit,
                    costs: self.costs,
                },
                GoalKind::TargetPrice => Goal::TargetPrice {
                    price: target_price.unwrap_or_default(),
                    without_card: self.target.without_card && self.source.has_card_price(),
                },
            },
            include_suspicious: self.include_suspicious,
            shop: ShopConditions {
                by_tomorrow: self.shop.by_tomorrow && self.source.has_delivery(),
                only_ozon: self.shop.only_ozon && self.source.has_marketplace_seller(),
            },
            new_only: self.new_only && self.source.has_used_goods(),
            every_minutes: self.every_minutes,
            hours: self.hours,
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
            link: Some("https://www.avito.ru/moskva/instrumenty?f=ASgBAgICAUT4vA2W0jQ".into()),
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
            shop: ShopConditions::default(),
            new_only: true,
            every_minutes: 20,
            hours: Some(Hours { from: 22, to: 7 }),
            enabled: false,
        };
        assert_eq!(WatchForm::edit(&watch).to_watch(), Ok(watch));
    }

    #[test]
    fn product_watch_survives_the_form_and_drops_what_it_does_not_use() {
        let headphones = Watch {
            id: 4,
            name: "Наушники к зиме".into(),
            link: Some("https://www.ozon.ru/product/sony-wh-ch520-895366703/".into()),
            goal: Goal::TargetPrice {
                price: 3_500,
                without_card: true,
            },
            ..Watch::new_draft()
        };
        assert_eq!(WatchForm::edit(&headphones).to_watch().as_ref(), Ok(&headphones));

        let mut form = WatchForm::edit(&headphones);
        form.source = SourceKind::Dns;
        form.radius_km = Some(5);
        let at_dns = form.to_watch().unwrap();
        assert_eq!(
            at_dns.goal,
            Goal::TargetPrice {
                price: 3_500,
                without_card: false
            },
            "у ДНС цена одна — галочка Ozon Карты не нужна"
        );
        assert_eq!(at_dns.radius_km, None, "товар по ссылке ищется не рядом с домом");
    }

    #[test]
    fn shop_conditions_keep_only_what_the_source_knows() {
        let fast_ozon = Watch {
            name: "Наушники".into(),
            query: "наушники sony".into(),
            shop: ShopConditions {
                by_tomorrow: true,
                only_ozon: true,
            },
            ..Watch::new_draft()
        };
        let mut form = WatchForm::edit(&fast_ozon);
        assert_eq!(form.to_watch().unwrap().shop, fast_ozon.shop);

        form.source = SourceKind::Dns;
        assert_eq!(
            form.to_watch().unwrap().shop,
            ShopConditions {
                by_tomorrow: true,
                only_ozon: false,
            },
            "у ДНС есть срок доставки, но нет сторонних продавцов"
        );

        form.source = SourceKind::Avito;
        assert_eq!(form.to_watch().unwrap().shop, ShopConditions::default());
    }

    #[test]
    fn watches_saved_before_the_rename_keep_their_conditions() {
        let mut saved = serde_json::to_value(Watch::new_draft()).unwrap();
        let fields = saved.as_object_mut().unwrap();
        fields.remove("shop");
        fields.insert(
            "ozon".into(),
            serde_json::json!({ "by_tomorrow": true, "only_ozon": false }),
        );

        let watch: Watch = serde_json::from_value(saved).unwrap();

        assert!(watch.shop.by_tomorrow);
    }
}
