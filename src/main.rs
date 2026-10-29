mod browser;
mod config;
mod export;
mod format;
mod geo;
mod gui;
mod health;
mod hunt;
mod instance;
mod marketplace;
mod offer;
mod quantity;
mod search;
mod settings;
mod storage;
mod telegram;
mod text;
mod wait;

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;
use teloxide::Bot;

use crate::browser::Browser;
use crate::config::Config;
use crate::health::ParserCheck;
use crate::hunt::{Hunter, Notifier};
use crate::marketplace::avito::Avito;
use crate::marketplace::dns::Dns;
use crate::marketplace::ozon::Ozon;
use crate::marketplace::{OfferSource, SourceError, SourceKind, Throttled};
use crate::search::OfferSearch;
use crate::settings::UserSettings;
use crate::storage::Storage;
use crate::telegram::TelegramAlerts;

/// Окно — в основном потоке; проверки слежек, Telegram и браузер — в фоне.
fn main() -> Result<()> {
    let app_dir = config::app_dir();
    dotenvy::from_path(app_dir.join(".env")).ok();
    init_logging();

    let data_dir = app_dir.join("data");
    let Some(_instance) = instance::claim(&data_dir)? else {
        log::warn!(
            "программа из {} уже запущена — второй экземпляр не запускается",
            app_dir.display()
        );
        return gui::already_running();
    };
    let storage = Storage::new(data_dir);
    let settings: UserSettings = storage.load(settings::FILE);
    let config = Config::load(&settings, &app_dir)?;
    let runtime = tokio::runtime::Runtime::new()?;
    let _inside_runtime = runtime.enter();

    let browser = Arc::new(Browser::new(config.browser)?);
    let ozon = Arc::new(Ozon::new(Arc::clone(&browser), config.ozon_portions));
    runtime.spawn(open_ozon(Arc::clone(&ozon)));
    let throttled = |source: Arc<dyn OfferSource>, kind: SourceKind| {
        Arc::new(Throttled::new(source, kind.search_gap())) as Arc<dyn OfferSource>
    };
    let ozon = throttled(ozon, SourceKind::Ozon);
    let avito = throttled(
        Arc::new(Avito::new(
            Arc::clone(&browser),
            settings.avito_region.clone(),
            storage.clone(),
        )),
        SourceKind::Avito,
    );
    let dns = throttled(Arc::new(Dns::new(browser)), SourceKind::Dns);

    let bot = config.telegram.as_ref().map(|telegram| Bot::new(&telegram.token));
    let owner = config.telegram.as_ref().and_then(|telegram| telegram.owner);
    let notifier = bot
        .clone()
        .zip(owner)
        .map(|(bot, owner)| Arc::new(TelegramAlerts::new(bot, owner)) as Arc<dyn Notifier>);

    let hunter = Arc::new(Hunter::new(
        HashMap::from([
            (SourceKind::Ozon, Arc::clone(&ozon)),
            (SourceKind::Avito, avito),
            (SourceKind::Dns, dns),
        ]),
        notifier,
        storage.clone(),
        settings.home,
    ));
    runtime.spawn(Arc::clone(&hunter).run());

    if let (Some(bot), Some(telegram)) = (bot, config.telegram) {
        let parser_check = config
            .self_check
            .map(|settings| ParserCheck::new(Arc::clone(&ozon), settings));
        runtime.spawn(telegram::run(
            bot,
            telegram,
            OfferSearch::new(ozon),
            Arc::clone(&hunter),
            parser_check,
        ));
    } else {
        log::warn!("Telegram не настроен: окно работает, уведомлений не будет");
    }

    gui::run(hunter, storage, settings, runtime.handle().clone())
}

async fn open_ozon(ozon: Arc<Ozon>) {
    match ozon.open().await {
        Ok(()) => log::info!("браузер с Ozon открыт"),
        Err(SourceError::Captcha | SourceError::NotReady) => {
            log::warn!("Ozon пока не открылся: пройдите проверку в окне браузера, если она есть");
        }
        Err(error) => log::error!("Ozon не открылся: {error:#}"),
    }
}

fn init_logging() {
    // Видеодрайверы шлют в лог «ошибки», которые на работу окна не влияют и только пугают.
    pretty_env_logger::formatted_timed_builder()
        .filter_level(log::LevelFilter::Info)
        .filter_module("wgpu_hal", log::LevelFilter::Off)
        .filter_module("wgpu_core", log::LevelFilter::Warn)
        .filter_module("egui_wgpu", log::LevelFilter::Warn)
        .filter_module("naga", log::LevelFilter::Warn)
        .parse_default_env()
        .init();
}
