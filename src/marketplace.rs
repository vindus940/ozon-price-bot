pub mod avito;
pub mod ozon;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::geo::GeoPoint;
use crate::offer::Offer;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SourceKind {
    Ozon,
    Avito,
}

impl SourceKind {
    pub const ALL: [SourceKind; 2] = [SourceKind::Ozon, SourceKind::Avito];

    pub fn title(self) -> &'static str {
        match self {
            SourceKind::Ozon => "Ozon",
            SourceKind::Avito => "Авито",
        }
    }

    /// Чаще проверять не стоит: площадка начнёт показывать проверку «я не робот».
    pub fn min_interval_minutes(self) -> u32 {
        match self {
            SourceKind::Ozon => 5,
            SourceKind::Avito => 15,
        }
    }

    /// Радиусы, которые площадка умеет искать вокруг точки и для которых пишет расстояние до объявления.
    /// Пусто — у площадки нет адресов товаров (Ozon везёт со склада).
    /// Авито проверено вживую (октябрь 2026): другие значения он молча отбрасывает.
    pub fn radii_km(self) -> &'static [u32] {
        match self {
            SourceKind::Ozon => &[],
            SourceKind::Avito => &[1, 2, 3, 5, 10, 15, 25],
        }
    }

    pub fn has_locations(self) -> bool {
        !self.radii_km().is_empty()
    }

    /// Срок доставки и метку «Ozon» знает только маркетплейс: у объявлений Авито их нет.
    pub fn has_delivery_and_seller(self) -> bool {
        self == SourceKind::Ozon
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
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;

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
    }
}
