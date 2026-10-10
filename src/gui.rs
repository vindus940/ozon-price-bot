mod deals;
mod journal;
mod settings_tab;
mod watch_form;
mod watches;

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, anyhow};
use eframe::egui::{self, Color32, RichText};

use crate::hunt::Hunter;
use crate::settings::UserSettings;
use crate::storage::Storage;
use settings_tab::SettingsTab;
use watch_form::{FormAction, WatchForm};

const TITLE: &str = "Охота за выгодой";
const WINDOWS_FONT: &str = r"C:\Windows\Fonts\segoeui.ttf";

const GOOD: Color32 = Color32::from_rgb(0x2e, 0x9e, 0x4f);
const WARN: Color32 = Color32::from_rgb(0xc9, 0x7a, 0x00);
const BAD: Color32 = Color32::from_rgb(0xd9, 0x36, 0x36);

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Watches,
    Deals,
    Settings,
    Journal,
}

struct HunterApp {
    hunter: Arc<Hunter>,
    tab: Tab,
    form: Option<WatchForm>,
    pending_delete: Option<u64>,
    deals_filter: Option<u64>,
    confirm_clear_deals: bool,
    settings: SettingsTab,
}

pub fn run(
    hunter: Arc<Hunter>,
    storage: Storage,
    settings: UserSettings,
    runtime: tokio::runtime::Handle,
) -> Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_inner_size([1180.0, 760.0])
            .with_min_inner_size([860.0, 520.0]),
        ..Default::default()
    };
    let app = HunterApp {
        hunter: Arc::clone(&hunter),
        tab: Tab::Watches,
        form: None,
        pending_delete: None,
        deals_filter: None,
        confirm_clear_deals: false,
        settings: SettingsTab::new(hunter, storage, settings, runtime),
    };
    eframe::run_native(
        TITLE,
        options,
        Box::new(|creation| {
            use_windows_font(&creation.egui_ctx);
            Ok(Box::new(app))
        }),
    )
    .map_err(|error| anyhow!("не удалось открыть окно: {error}"))
}

impl eframe::App for HunterApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Проверки идут в фоне: раз в секунду окно перерисовывается, чтобы показать свежие статусы.
        ui.ctx().request_repaint_after(Duration::from_secs(1));

        egui::Panel::top("status").show(ui, |ui| self.status_bar(ui));
        egui::Panel::left("navigation")
            .resizable(false)
            .exact_size(170.0)
            .show(ui, |ui| self.navigation(ui));
        egui::CentralPanel::default().show(ui, |ui| match self.tab {
            Tab::Watches => watches::show(ui, &self.hunter, &mut self.form, &mut self.pending_delete),
            Tab::Deals => deals::show(ui, &self.hunter, &mut self.deals_filter, &mut self.confirm_clear_deals),
            Tab::Settings => self.settings.show(ui),
            Tab::Journal => journal::show(ui, &self.hunter),
        });

        let ctx = ui.ctx().clone();
        self.watch_form_window(&ctx);
    }
}

impl HunterApp {
    fn status_bar(&self, ui: &mut egui::Ui) {
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.heading(TITLE);
            ui.separator();
            let watches = self.hunter.watches();
            let enabled = watches.iter().filter(|watch| watch.enabled).count();
            ui.label(format!("Слежек включено: {enabled} из {}", watches.len()));
            ui.separator();
            ui.label(format!("Находок: {}", self.hunter.deals_count()));
            ui.separator();
            if self.hunter.notifications_enabled() {
                ui.colored_label(GOOD, "Telegram: уведомления включены");
            } else {
                ui.colored_label(WARN, "Telegram: уведомления не настроены — см. «Настройки»");
            }
        });
        ui.add_space(4.0);
    }

    fn navigation(&mut self, ui: &mut egui::Ui) {
        ui.add_space(10.0);
        for (tab, title) in [
            (Tab::Watches, "Слежки"),
            (Tab::Deals, "Находки"),
            (Tab::Settings, "Настройки"),
            (Tab::Journal, "Журнал"),
        ] {
            if ui
                .selectable_label(self.tab == tab, RichText::new(title).size(17.0))
                .clicked()
            {
                self.tab = tab;
            }
            ui.add_space(6.0);
        }
    }

    fn watch_form_window(&mut self, ctx: &egui::Context) {
        let home_is_set = self.hunter.home().is_some();
        let hunter = Arc::clone(&self.hunter);
        let Some(form) = self.form.as_mut() else { return };

        let mut open = true;
        let mut action = FormAction::Keep;
        egui::Window::new(form.title())
            .collapsible(false)
            .resizable(false)
            .default_width(600.0)
            .open(&mut open)
            .show(ctx, |ui| {
                action = form.show(ui, home_is_set, |source| hunter.is_connected(source));
            });

        let close = match action {
            FormAction::Save => match form.to_watch().and_then(|watch| hunter.save_watch(watch)) {
                Ok(_) => true,
                Err(problems) => {
                    form.problems = problems;
                    false
                }
            },
            FormAction::Cancel => true,
            FormAction::Keep => !open,
        };
        if close {
            self.form = None;
        }
    }
}

/// Segoe UI — системный шрифт Windows: в нём есть и кириллица, и знак рубля.
fn use_windows_font(ctx: &egui::Context) {
    let Ok(bytes) = std::fs::read(WINDOWS_FONT) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts
        .font_data
        .insert("segoe".to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "segoe".to_owned());
    ctx.set_fonts(fonts);
}
