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

    /// Страницу одного товара читают магазины. У объявлений Авито её не читаем: каждая проверка — лишний переход
    /// на сайт, который и так ограничивал адрес за частые заходы.
    pub fn has_product_pages(self) -> bool {
        matches!(self, SourceKind::Ozon | SourceKind::Dns)
    }

    /// Две цены — с картой магазина и без неё — у Ozon (Ozon Карта); у ДНС цена одна.
    pub fn has_card_price(self) -> bool {
        self == SourceKind::Ozon
    }

    /// Ссылка на страницу товара для слежки за его ценой: у Ozon и ДНС адрес товара начинается с `/product/`.
    /// Параметры после `?` — метки, откуда пришли, цене они не нужны.
    pub fn parse_product_link(self, text: &str) -> Result<Url, String> {
        if !self.has_product_pages() {
            return Err(format!(
                "Следить за ценой товара по ссылке можно на Ozon и в ДНС, у {} — нет",
                self.title()
            ));
        }
        let mut link = self.parse_link(text)?;
        if !link.path().starts_with("/product/") {
            return Err(
                "Это ссылка не на товар — откройте страницу товара и скопируйте адрес из адресной строки".to_string(),
            );
        }
        link.set_query(None);
        link.set_fragment(None);
        Ok(link)
    }

    /// Выдача от дешёвых к дорогим в коридоре цен `from..=to` — по запросу или поверх ссылки слежки с её фильтрами.
    /// Ozon понимает `sorting=price` и `currency_price=3000.000;5000.000` (без «.000» граница молча не применяется,
    /// 29.10.2026); листалка оба параметра сохраняет. Без нижней границы первыми идут чехлы за 100 ₽.
    /// `None` — площадка так не умеет (у Авито и ДНС сортировку по цене не проверяли).
    pub fn cheapest_first(self, query: &str, link: Option<&Url>, from: u64, to: u64) -> Option<Url> {
        if self != SourceKind::Ozon {
            return None;
        }
        let mut url = match link {
            Some(link) => link.clone(),
            None => Url::parse_with_params(&format!("https://{}/search/", self.site()), [("text", query)]).ok()?,
        };
        let kept: Vec<(String, String)> = url
            .query_pairs()
            .filter(|(key, _)| !matches!(key.as_ref(), "sorting" | "currency_price" | "page"))
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        url.query_pairs_mut()
            .clear()
            .extend_pairs(&kept)
            .append_pair("sorting", "price")
            .append_pair("currency_price", &format!("{from}.000;{to}.000"));
        Some(url)
    }
}

/// Товар со своей страницы. `price` — цена, которую площадка показывает первой (у Ozon — с Ozon Картой);
/// `None` — товара нет в продаже. `price_without_card` — цена для тех, у кого Ozon Карты нет; есть только у Ozon.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Product {
    pub name: String,
    pub price: Option<u64>,
    pub price_without_card: Option<u64>,
}

/// Сайт в браузере программы, как он виден со стороны: браузер не запущен, запущен без вкладки сайта или с ней.
/// Закрытый браузер — не поломка: он откроется сам при следующем поиске.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteStatus {
    BrowserClosed,
    NotOpened,
    Open,
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

    /// Цена одного товара по ссылке на его страницу (`SourceKind::parse_product_link`).
    async fn product(&self, link: &Url) -> Result<Product, SourceError>;
}

#[cfg(test)]
pub mod testing {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use reqwest::Url;

    use super::{Area, OfferSource, Product, SourceError};
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

        async fn product(&self, link: &Url) -> Result<Product, SourceError> {
            self.queries.lock().unwrap().push(link.to_string());
            self.offers.as_ref().ok_or(SourceError::Captcha)?;
            Err(anyhow::anyhow!("у подставной площадки нет страниц товаров").into())
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

    #[test]
    fn cheapest_first_sorts_by_price_within_the_corridor_and_keeps_link_filters() {
        let by_query = SourceKind::Ozon
            .cheapest_first("наушники sony", None, 2_200, 3_500)
            .unwrap();
        assert_eq!(
            by_query.as_str(),
            "https://www.ozon.ru/search/?text=%D0%BD%D0%B0%D1%83%D1%88%D0%BD%D0%B8%D0%BA%D0%B8+sony\
             &sorting=price&currency_price=2200.000%3B3500.000"
        );

        let link = SourceKind::Ozon
            .parse_link("https://www.ozon.ru/category/audiotehnika-15543/?brand=32686750&sorting=rating&page=3")
            .unwrap();
        let by_link = SourceKind::Ozon
            .cheapest_first("наушники", Some(&link), 1_000, 2_000)
            .unwrap();
        assert_eq!(
            by_link.as_str(),
            "https://www.ozon.ru/category/audiotehnika-15543/?brand=32686750&sorting=price&currency_price=1000.000%3B2000.000",
            "свой порядок и страница ссылки заменяются, фильтр бренда остаётся"
        );

        assert!(SourceKind::Dns.cheapest_first("наушники", None, 1_000, 2_000).is_none());
    }

    #[test]
    fn product_link_leads_to_a_product_page_of_a_shop() {
        let link = SourceKind::Ozon
            .parse_product_link("https://ozon.ru/product/sony-wh-ch520-895366703/?at=abc&from=share#reviews")
            .unwrap();
        assert_eq!(link.as_str(), "https://www.ozon.ru/product/sony-wh-ch520-895366703/");

        let search = SourceKind::Dns
            .parse_product_link("https://www.dns-shop.ru/search/?q=sony")
            .unwrap_err();
        assert!(search.starts_with("Это ссылка не на товар"), "{search}");

        let avito = SourceKind::Avito
            .parse_product_link("https://www.avito.ru/moskva/audio_i_video/naushniki_8284433652")
            .unwrap_err();
        assert!(avito.contains("на Ozon и в ДНС"), "{avito}");
    }
}
