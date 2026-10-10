use std::env;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Result, anyhow};

use crate::browser::BrowserSettings;
use crate::health::SelfCheckSettings;
use crate::settings::UserSettings;
use crate::telegram::TelegramSettings;

pub(crate) const DEFAULT_BROWSER: &str = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
const DEFAULT_SELF_CHECK_QUERY: &str = "наушники";

pub struct Config {
    /// `None` — токена нет ни в настройках программы, ни в `.env`: окно работает, уведомлений нет.
    pub telegram: Option<TelegramSettings>,
    pub browser: BrowserSettings,
    pub ozon_portions: u32,
    pub self_check: Option<SelfCheckSettings>,
}

impl Config {
    /// Токен и ID из окна программы важнее `.env`; остальное — из `.env`.
    pub fn load(user: &UserSettings) -> Result<Self> {
        let users = owner_first(
            user.telegram_user_id,
            user_ids(&env::var("ALLOWED_USERS").unwrap_or_default())?,
        );
        let token = Some(user.telegram_token.trim().to_string())
            .filter(|token| !token.is_empty())
            .or_else(|| env::var("TELEGRAM_TOKEN").ok().filter(|token| !token.trim().is_empty()));
        let top = number("TOP", 10)?;

        Ok(Self {
            telegram: token.map(|token| TelegramSettings {
                token,
                owner: users.first().copied(),
                allowed_users: users.into_iter().collect(),
                top,
            }),
            browser: BrowserSettings {
                executable: env::var("BROWSER_PATH")
                    .unwrap_or_else(|_| DEFAULT_BROWSER.into())
                    .into(),
                debug_port: number("DEBUG_PORT", 9222)?,
                profile_dir: env::current_dir()?.join("browser-profile"),
            },
            ozon_portions: number("PORTIONS", 10)?,
            self_check: self_check()?,
        })
    }
}

fn owner_first(owner: Option<u64>, mut users: Vec<u64>) -> Vec<u64> {
    if let Some(owner) = owner {
        users.retain(|&user| user != owner);
        users.insert(0, owner);
    }
    users
}

fn self_check() -> Result<Option<SelfCheckSettings>> {
    let hours: u64 = number("SELF_CHECK_HOURS", 6)?;
    Ok((hours > 0).then(|| SelfCheckSettings {
        every: Duration::from_hours(hours),
        query: env::var("SELF_CHECK_QUERY").unwrap_or_else(|_| DEFAULT_SELF_CHECK_QUERY.into()),
    }))
}

fn number<T: FromStr>(name: &str, default: T) -> Result<T> {
    match env::var(name) {
        Ok(value) => value
            .trim()
            .parse()
            .map_err(|_| anyhow!("{name} в .env должно быть числом, а сейчас «{value}»")),
        Err(_) => Ok(default),
    }
}

fn user_ids(list: &str) -> Result<Vec<u64>> {
    list.split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| {
            id.parse()
                .map_err(|_| anyhow!("в ALLOWED_USERS «{id}» — не Telegram ID"))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_ids_keep_order_and_reject_typos() {
        assert_eq!(user_ids(" 123, 456 ,").unwrap(), [123, 456]);
        assert!(user_ids("").unwrap().is_empty());
        assert!(user_ids("123, abc").is_err());
    }

    #[test]
    fn id_from_program_settings_becomes_owner_without_duplicates() {
        assert_eq!(owner_first(Some(456), vec![123, 456]), [456, 123]);
        assert_eq!(owner_first(None, vec![123, 456]), [123, 456]);
        assert_eq!(owner_first(Some(7), Vec::new()), [7]);
    }
}
