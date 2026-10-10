use serde::{Deserialize, Serialize};

use crate::geo::GeoPoint;

pub const FILE: &str = "settings.json";
const DEFAULT_AVITO_REGION: &str = "moskva";

/// Настройки, которые меняются из окна программы. Технические (путь к браузеру, порт) остаются в `.env`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UserSettings {
    pub telegram_token: String,
    pub telegram_user_id: Option<u64>,
    pub home: Option<GeoPoint>,
    /// Город на Авито, как в адресе сайта: moskva, sankt-peterburg. Радиус Авито работает только внутри города.
    pub avito_region: String,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            telegram_token: String::new(),
            telegram_user_id: None,
            home: None,
            avito_region: DEFAULT_AVITO_REGION.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_saved_before_avito_get_moscow() {
        let old: UserSettings =
            serde_json::from_str(r#"{ "telegram_token": "t", "telegram_user_id": 7, "home": null }"#).unwrap();
        assert_eq!(old.avito_region, "moskva");
        assert_eq!(old.telegram_user_id, Some(7));
    }
}
