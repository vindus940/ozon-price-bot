use std::time::Duration;

use anyhow::anyhow;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::time::{Instant, sleep};

use crate::browser::Tab;
use crate::marketplace::SourceError;

const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(700);
const MARK_PAGE_STALE: &str = "window.__huntStale = true";

/// Что скрипт площадки видит на открытой странице.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageState {
    Loading,
    Captcha,
    NotFound,
    /// Товары или «ничего не найдено» — выдача готова.
    Listings,
}

/// Страница выдачи, прочитанная скриптом площадки.
pub trait ScriptPage: DeserializeOwned {
    /// Это ещё старая страница: программа помечает её перед переходом, а новая пометки не знает.
    fn stale(&self) -> bool;
    fn state(&self) -> &PageState;
}

/// Некоторые площадки (Авито, ДНС) отдают выдачу только при настоящем переходе: запрос из скрипта получает
/// пустой каркас. Поэтому вкладка открывает адрес, как человек, а скрипт площадки ждёт готовых карточек.
pub async fn open<P: ScriptPage>(tab: &Tab, url: &str, script: &str, not_found: &str) -> Result<P, SourceError> {
    tab.evaluate::<bool>(MARK_PAGE_STALE).await?;
    tab.navigate(url).await?;
    let deadline = Instant::now() + PAGE_TIMEOUT;
    loop {
        sleep(POLL_INTERVAL).await;
        if let Ok(page) = tab.evaluate::<P>(script).await
            && !page.stale()
        {
            match page.state() {
                PageState::Loading => {}
                PageState::Captcha => return Err(SourceError::Captcha),
                PageState::NotFound => return Err(anyhow!("{not_found}").into()),
                PageState::Listings => return Ok(page),
            }
        }
        if Instant::now() >= deadline {
            return Err(SourceError::NotReady);
        }
    }
}
