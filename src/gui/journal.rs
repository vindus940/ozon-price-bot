use eframe::egui;

use crate::hunt::Hunter;

pub(super) fn show(ui: &mut egui::Ui, hunter: &Hunter) {
    ui.heading("Журнал");
    ui.weak("Что и когда делала программа: проверки слежек, сохранения, ошибки. Свежее — сверху.");
    ui.add_space(8.0);

    let entries = hunter.journal();
    if entries.is_empty() {
        ui.label("Пока пусто — записи появятся после первой проверки.");
        return;
    }
    egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
        for entry in entries {
            ui.horizontal_wrapped(|ui| {
                ui.weak(entry.at.format("%d.%m %H:%M:%S").to_string());
                ui.label(entry.text);
            });
        }
    });
}
