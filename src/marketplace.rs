pub mod avito;
pub mod dns;
pub mod ozon;
mod page;
mod throttle;

pub use throttle::Throttled;

use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::geo::GeoPoint;
use crate::offer::Offer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceKind {
    Ozon,
    Avito,
    Dns,
}

impl SourceKind {
    pub const ALL: [SourceKind; 3] = [SourceKind::Ozon, SourceKind::Avito, SourceKind::Dns];

    pub fn title(self) -> &'static str {
        match self {
            SourceKind::Ozon => "Ozon",
            SourceKind::Avito => "Авито",
            SourceKind::Dns => "ДНС",
        }
    }

    /// Чаще проверять не стоит: площадка начнёт показывать проверку «я не робот».
    /// ДНС закрыт антибот-защитой строже Ozon — проверки реже.
    pub fn min_interval_minutes(self) -> u32 {
        match self {
            SourceKind::Ozon => 5,
            SourceKind::Avito | SourceKind::Dns => 15,
        }
    }

    /// Самое частое, с чем площадка мирится без проверки «я не робот», — общий предел для слежек, бота
    /// и самопроверки. Один поиск Ozon — это ещё и до десятка порций выдачи, Авито и ДНС открывают страницы целиком.
    pub fn search_gap(self) -> Duration {
        let searches_per_minute = match self {
            SourceKind::Ozon => 4,
            SourceKind::Avito | SourceKind::Dns => 2,
        };
        Duration::from_mins(1) / searches_per_minute
    }

    /// Радиусы, которые площадка умеет искать вокруг точки и для которых пишет расстояние до объявления.
    /// Пусто — у площадки нет адресов товаров (Ozon везёт со склада).
    /// Авито проверено вживую (октябрь 2026): другие значения он молча отбрасывает.
    pub fn radii_km(self) -> &'static [u32] {
        match self {
            SourceKind::Ozon | SourceKind::Dns => &[],
            SourceKind::Avito => &[1, 2, 3, 5, 10, 15, 25],
        }
    }

    pub fn has_locations(self) -> bool {
        !self.radii_km().is_empty()
    }

    /// Срок доставки пишут магазины — Ozon и ДНС; у объявлений Авито его нет.
    pub fn has_delivery(self) -> bool {
        matches!(self, SourceKind::Ozon | SourceKind::Dns)
    }

    /// Метка «продаёт сам Ozon» бывает только у маркетплейса со сторонними продавцами.
    pub fn has_marketplace_seller(self) -> bool {
        self == SourceKind::Ozon
    }

    /// Б/у продают частники на Авито; магазины продают новое.
    pub fn has_used_goods(self) -> bool {
        self == SourceKind::Avito
    }

    /// Что важно знать о площадке, выбирая её для слежки.
    pub fn note(self) -> Option<&'static str> {
        match self {
            SourceKind::Dns => Some(
                "Цены и наличие ДНС — для города, выбранного на dns-shop.ru в окне браузера программы: \
                 выберите там свой город один раз",
            ),
            SourceKind::Ozon | SourceKind::Avito => None,
        }
    }

    pub fn site(self) -> &'static str {
        match self {
            SourceKind::Ozon => "www.ozon.ru",
            SourceKind::Avito => "www.avito.ru",
            SourceKind::Dns => "www.dns-shop.ru",
        }
    }

    /// Ссылка на выдачу с фильтрами, настроенными на самом сайте: из неё слежка берёт то, что программа сама
    /// задать не умеет (у Авито «Состояние» своё в каждой категории). Адрес без `www.` приводится к нему:
    /// так сайт открывается в окне браузера программы. Ошибка — понятная причина для формы слежки.
    pub fn parse_link(self, text: &str) -> Result<Url, String> {
        let not_a_link =
            || "Ссылка не похожа на адрес страницы — скопируйте её из адресной строки браузера".to_string();
        let mut link = Url::parse(text.trim()).map_err(|_| not_a_link())?;
        let host = link.host_str().unwrap_or_default().to_string();
        let own_site = host == self.site() || format!("www.{host}") == self.site();
        if !matches!(link.scheme(), "https" | "http") || host.is_empty() {
            return Err(not_a_link());
        }
        if !own_site {
            return Err(format!("Ссылка должна вести на {}, а эта ведёт на {host}", self.site()));
        }
        link.set_scheme("https").map_err(|()| not_a_link())?;
        link.set_host(Some(self.site())).map_err(|_| not_a_link())?;
        Ok(link)
    }
}

/// Где искать: вокруг точки в радиусе. Площадки без адресов товаров его не учитывают.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Area {
    pub center: GeoPoint,
    pub radius_km: u32,
}

#[derive(Debug, Error)]
pub enum SourceError {
    #[error("площадка просит подтвердить, что вы не робот")]
    Captcha,
    #[error("страница площадки не открылась")]
    NotReady,
    #[error(transparent)]
    Failed(#[from] anyhow::Error),
}

/// Площадка, на которой ищутся предложения.
///
/// Возвращает всё найденное по запросу как есть — отбор и сортировка не её забота.
/// У каждого предложения цена больше нуля, а ссылка абсолютная и без параметров отслеживания.
/// Если передана область и площадка знает адреса товаров, у предложений заполнено `distance_km` от её центра.
#[async_trait]
pub trait OfferSource: Send + Sync {
    fn name(&self) -> &'static str;

    async fn search(&self, query: &str, area: Option<Area>) -> Result<Vec<Offer>, SourceError>;

    /// То же, но по ссылке на выдачу с фильтрами, настроенными на самом сайте (`SourceKind::parse_link`).
    async fn search_link(&self, link: &Url, area: Option<Area>) -> Result<Vec<Offer>, SourceError>;
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reqwest::Url;

    use super::{Area, OfferSource, SourceError};
    use crate::offer::Offer;

    /// Площадка для тестов: отдаёт заданные предложения или просит проверку «я не робот» и запоминает запросы.
    pub struct FakeSource {
        offers: Option<Vec<Offer>>,
        queries: Arc<Mutex<Vec<String>>>,
    }

    impl FakeSource {
        pub fn returning(offers: Vec<Offer>) -> Self {
            Self {
                offers: Some(offers),
                queries: Arc::default(),
            }
        }

        pub fn asking_captcha() -> Self {
            Self {
                offers: None,
                queries: Arc::default(),
            }
        }

        pub fn queries(&self) -> Arc<Mutex<Vec<String>>> {
            Arc::clone(&self.queries)
        }
    }

    #[async_trait]
    impl OfferSource for FakeSource {
        fn name(&self) -> &'static str {
            "Fake"
        }

        async fn search(&self, query: &str, _area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
            self.queries.lock().unwrap().push(query.to_string());
            self.offers.clone().ok_or(SourceError::Captcha)
        }

        async fn search_link(&self, link: &Url, area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
            self.search(link.as_str(), area).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_must_lead_to_the_source_site() {
        let link = SourceKind::Avito
            .parse_link(" https://avito.ru/moskva/bytovaya_elektronika?f=ASgBAgICAUT4vA2W0jQ&q=g435 ")
            .unwrap();
        assert_eq!(
            link.as_str(),
            "https://www.avito.ru/moskva/bytovaya_elektronika?f=ASgBAgICAUT4vA2W0jQ&q=g435",
            "без www тоже подходит, лишние пробелы отрезаются"
        );

        let wrong_site = SourceKind::Ozon
            .parse_link("https://www.avito.ru/moskva?q=g435")
            .unwrap_err();
        assert_eq!(
            wrong_site,
            "Ссылка должна вести на www.ozon.ru, а эта ведёт на www.avito.ru"
        );
        assert!(
            SourceKind::Dns.parse_link("dns-shop.ru/catalog/").is_err(),
            "без https:// — не адрес"
        );
        assert!(SourceKind::Ozon.parse_link("https://www.ozon.ru.example.com/").is_err());
    }
}
