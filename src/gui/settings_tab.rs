use std::num::ParseIntError;
use std::sync::{Arc, Mutex};

use eframe::egui::{self, Color32, RichText};

use super::{BAD, GOOD, WARN};
use crate::geo::GeoPoint;
use crate::hunt::Hunter;
use crate::marketplace::SourceKind;
use crate::settings::{self, UserSettings};
use crate::storage::Storage;

pub(super) struct SettingsTab {
    hunter: Arc<Hunter>,
    storage: Storage,
    runtime: tokio::runtime::Handle,
    saved: UserSettings,
    home: String,
    token: String,
    user_id: String,
    avito_region: String,
    notice: Option<(String, Color32)>,
    test_result: Arc<Mutex<Option<String>>>,
}

impl SettingsTab {
    pub(super) fn new(
        hunter: Arc<Hunter>,
        storage: Storage,
        saved: UserSettings,
        runtime: tokio::runtime::Handle,
    ) -> Self {
        Self {
            hunter,
            storage,
            runtime,
            home: saved.home.map(|home| home.to_string()).unwrap_or_default(),
            token: saved.telegram_token.clone(),
            user_id: saved.telegram_user_id.map(|id| id.to_string()).unwrap_or_default(),
            avito_region: saved.avito_region.clone(),
            saved,
            notice: None,
            test_result: Arc::default(),
        }
    }

    pub(super) fn show(&mut self, ui: &mut egui::Ui) {
        ui.heading("Настройки");
        ui.add_space(8.0);
        egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
            self.home_section(ui);
            ui.add_space(10.0);
            self.telegram_section(ui);
            ui.add_space(10.0);
            self.avito_section(ui);
            ui.add_space(10.0);
            section(ui, "Где хранятся данные", |ui| {
                ui.label(format!(
                    "Слежки, находки и настройки — в папке {}",
                    self.storage.dir().display()
                ));
                ui.weak(
                    "Технические настройки (путь к браузеру, порт, самопроверка) — в файле .env рядом с программой.",
                );
            });
            if let Some((text, color)) = &self.notice {
                ui.add_space(10.0);
                ui.colored_label(*color, text);
            }
        });
    }

    fn home_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Дом", |ui| {
            ui.label("От этой точки Авито считает расстояние до объявлений — для слежек «рядом с домом».");
            ui.add(
                egui::TextEdit::singleline(&mut self.home)
                    .hint_text("55.751244, 37.618423")
                    .desired_width(260.0),
            );
            ui.weak("В Яндекс Картах нажмите правой кнопкой на свой дом, выберите «Что здесь?» и скопируйте координаты сюда.");

            let parsed = GeoPoint::parse(&self.home);
            match parsed {
                Some(point) => ui.colored_label(GOOD, format!("Понял: {point}")),
                None if self.home.trim().is_empty() => ui.weak("Не указан"),
                None => ui.colored_label(BAD, "Не похоже на координаты: нужны два числа через запятую"),
            };
            if ui.button("Сохранить дом").clicked() && (parsed.is_some() || self.home.trim().is_empty()) {
                self.saved.home = parsed;
                self.hunter.set_home(parsed);
                self.save("Дом сохранён");
            }
        });
    }

    fn telegram_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Telegram", |ui| {
            if self.hunter.notifications_enabled() {
                ui.colored_label(GOOD, "Уведомления работают.");
            } else {
                ui.colored_label(WARN, "Уведомления не настроены: нужен токен бота и ваш Telegram ID.");
            }
            egui::Grid::new("telegram")
                .num_columns(2)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    ui.label("Токен бота");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.token)
                            .password(true)
                            .hint_text("выдаёт @BotFather")
                            .desired_width(380.0),
                    );
                    ui.end_row();
                    ui.label("Ваш Telegram ID");
                    ui.add(
                        egui::TextEdit::singleline(&mut self.user_id)
                            .hint_text("бот пришлёт его в ответ на любое сообщение")
                            .desired_width(380.0),
                    );
                    ui.end_row();
                });

            ui.horizontal(|ui| {
                if ui.button("Сохранить").clicked() {
                    self.save_telegram();
                }
                let test = ui.add_enabled(
                    self.hunter.notifications_enabled(),
                    egui::Button::new("Отправить пробное уведомление"),
                );
                if test.clicked() {
                    self.send_test();
                }
            });
            if let Some(result) = self
                .test_result
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
            {
                ui.label(result);
            }
            ui.weak("Новый токен или ID начнут действовать после перезапуска программы.");
        });
    }

    fn avito_section(&mut self, ui: &mut egui::Ui) {
        section(ui, "Авито", |ui| {
            if self.hunter.is_connected(SourceKind::Avito) {
                ui.colored_label(GOOD, "Подключено.");
            } else {
                ui.colored_label(WARN, "Пока не подключено.");
            }
            ui.label(
                "Объявления читаются в отдельном окне браузера программы, не чаще раза в 15 минут. \
                 Если Авито покажет там проверку «я не робот», пройдите её сами. Входить в этом окне в свой аккаунт Авито не нужно.",
            );
            ui.horizontal(|ui| {
                ui.label("Город, как в адресе сайта");
                ui.add(
                    egui::TextEdit::singleline(&mut self.avito_region)
                        .hint_text("moskva")
                        .desired_width(180.0),
                );
                if ui.button("Сохранить город").clicked() {
                    self.save_avito_region();
                }
            });
            ui.weak(
                "Откройте avito.ru, выберите свой город и посмотрите адресную строку: avito.ru/moskva → moskva. \
                 Поиск рядом с домом работает внутри этого города. Новый город начнёт действовать после перезапуска программы.",
            );
        });
    }

    fn save_avito_region(&mut self) {
        let Some(region) = avito_region(&self.avito_region) else {
            self.notice = Some((
                "Город на Авито — латиницей, как в адресе сайта, например moskva".into(),
                BAD,
            ));
            return;
        };
        self.saved.avito_region = region;
        self.save("Город сохранён. Перезапустите программу, чтобы он начал действовать.");
    }

    fn save_telegram(&mut self) {
        let Ok(user_id) = telegram_user_id(&self.user_id) else {
            self.notice = Some(("Telegram ID — это число, например 123456789".into(), BAD));
            return;
        };
        self.saved.telegram_token = self.token.trim().to_string();
        self.saved.telegram_user_id = user_id;
        self.save("Сохранено. Перезапустите программу, чтобы новый токен или ID начали действовать.");
    }

    fn send_test(&self) {
        let hunter = Arc::clone(&self.hunter);
        let result = Arc::clone(&self.test_result);
        self.runtime.spawn(async move {
            let text = match hunter.send_test().await {
                Ok(()) => "Отправлено — проверьте Telegram.".to_string(),
                Err(error) => format!("Не отправилось: {error:#}"),
            };
            *result.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text);
        });
    }

    fn save(&mut self, success: &str) {
        self.notice = Some(match self.storage.save(settings::FILE, &self.saved) {
            Ok(()) => (success.to_string(), GOOD),
            Err(error) => (format!("Не удалось сохранить: {error:#}"), BAD),
        });
    }
}

fn section(ui: &mut egui::Ui, title: &str, content: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).inner_margin(12.0).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new(title).strong().size(16.0));
        ui.add_space(4.0);
        content(ui);
    });
}

/// Город на Авито — латиницей, как в адресе сайта: avito.ru/moskva → moskva. Он попадает в адрес поиска как есть.
fn avito_region(input: &str) -> Option<String> {
    let region = input.trim().to_lowercase();
    let in_address_form = !region.is_empty()
        && region
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    in_address_form.then_some(region)
}

/// Пустое поле — ID не задан, и уведомлений не будет; иначе — только число.
fn telegram_user_id(input: &str) -> Result<Option<u64>, ParseIntError> {
    let input = input.trim();
    if input.is_empty() {
        Ok(None)
    } else {
        input.parse().map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avito_city_is_taken_as_in_the_site_address() {
        assert_eq!(avito_region(" Moskva "), Some("moskva".into()));
        assert_eq!(avito_region("sankt-peterburg"), Some("sankt-peterburg".into()));
        assert_eq!(avito_region("москва"), None);
        assert_eq!(avito_region("moskva/all"), None);
        assert_eq!(avito_region("   "), None);
    }

    #[test]
    fn telegram_id_is_a_number_or_nothing() {
        assert_eq!(telegram_user_id(""), Ok(None));
        assert_eq!(telegram_user_id(" 123456789 "), Ok(Some(123_456_789)));
        assert!(telegram_user_id("@my_name").is_err());
    }
}
