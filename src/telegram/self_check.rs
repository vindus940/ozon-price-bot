use std::time::Duration;

use teloxide::prelude::*;
use teloxide::types::{ChatId, UserId};
use tokio::time::{Instant, MissedTickBehavior, interval_at};

use super::render;
use crate::health::{Health, HealthMonitor, ParserCheck};

// Минута после старта — чтобы успеть пройти проверку «я не робот», если браузер открылся с ней.
const FIRST_CHECK_DELAY: Duration = Duration::from_mins(1);

pub(super) async fn watch(bot: Bot, owner: Option<UserId>, check: ParserCheck) {
    let mut schedule = interval_at(Instant::now() + FIRST_CHECK_DELAY, check.every());
    // После сна компьютера — одна проверка, а не пачка пропущенных подряд.
    schedule.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let mut monitor = HealthMonitor::default();

    loop {
        schedule.tick().await;
        let health = check.run().await;
        log_result(&health);
        if monitor.changed(&health) {
            notify(&bot, owner, render::health(&health, check.source_name(), check.query())).await;
        }
    }
}

async fn notify(bot: &Bot, owner: Option<UserId>, text: String) {
    let Some(owner) = owner else {
        log::warn!("результат самопроверки некому отправить: ALLOWED_USERS пуст");
        return;
    };
    if let Err(error) = bot.send_message(ChatId::from(owner), text).await {
        log::warn!("не удалось отправить результат самопроверки: {error}");
    }
}

fn log_result(health: &Health) {
    match health {
        Health::Working { offers } => log::info!("самопроверка: найдено товаров {offers}"),
        Health::Broken { problems, .. } => log::warn!("самопроверка: разбор выдачи сломан: {problems:?}"),
        Health::Unavailable(error) => log::warn!("самопроверка не прошла: {error:#}"),
    }
}
