use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use regex::Regex;
use reqwest::Url;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::browser::{Browser, Tab};
use crate::marketplace::page::{self, PageState, ScriptPage};
use crate::marketplace::{Area, OfferSource, SourceError};
use crate::offer::Offer;

const HOME: &str = "https://www.avito.ru/";
const READ_SCRIPT: &str = include_str!("avito/read.js");
/// Сортировка «по дате»: свежие объявления первыми — ради них и проверяем.
const NEWEST_FIRST: &str = "104";
/// Так Авито отвечает, например, на опечатку в городе: avito.ru/moskwa.
const NOT_FOUND: &str = "Авито ответил «страница не найдена» — проверьте город в «Настройках»";

/// Авито отдаёт выдачу только при настоящем переходе на страницу (запрос из скрипта получает пустой каркас),
/// поэтому вкладка открывает страницу поиска, как человек, и читает готовые карточки.
pub struct Avito {
    browser: Arc<Browser>,
    region: String,
    /// Одна вкладка — один поиск за раз. Под тем же замком — запомненные категории запросов.
    category_paths: Mutex<HashMap<String, String>>,
}

#[derive(Deserialize)]
struct Page {
    stale: bool,
    state: PageState,
    path: String,
    radius: Option<String>,
    cards: Vec<Card>,
}

#[derive(Deserialize)]
struct Card {
    title: String,
    url: String,
    price: Option<String>,
    place: Option<String>,
    posted: Option<String>,
    seller_rating: Option<String>,
    seller_reviews: Option<String>,
    text: Option<String>,
}

impl Avito {
    /// `region` — город, как в адресе сайта: moskva, sankt-peterburg, ekaterinburg.
    pub fn new(browser: Arc<Browser>, region: impl Into<String>) -> Self {
        Self {
            browser,
            region: region.into(),
            category_paths: Mutex::new(HashMap::new()),
        }
    }

    /// Поиск по словам Авито сам переводит в категорию («шуруповерт» → «Дрели и шуруповёрты»), и радиус
    /// работает только в адресе категории. Категория запоминается, чтобы не открывать лишнюю страницу.
    async fn category_path(
        &self,
        tab: &Tab,
        paths: &mut HashMap<String, String>,
        query: &str,
    ) -> Result<String, SourceError> {
        if let Some(path) = paths.get(query) {
            return Ok(path.clone());
        }
        let url = Url::parse_with_params(&format!("{HOME}{}", self.region), &[("q", query)])
            .context("не удалось составить адрес поиска")?;
        open(tab, url.as_str()).await?;
        let path: String = tab.evaluate("location.pathname").await?;
        paths.insert(query.to_string(), path.clone());
        Ok(path)
    }
}

#[async_trait]
impl OfferSource for Avito {
    fn name(&self) -> &'static str {
        "Авито"
    }

    async fn search(&self, query: &str, area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
        let mut paths = self.category_paths.lock().await;
        let tab = self.browser.tab(HOME).await?;
        let path = self.category_path(&tab, &mut paths, query).await?;
        let mut page = open(&tab, search_url(&path, query, area)?.as_str()).await?;

        if let Some(area) = area
            && !page.has_radius(area)
        {
            // Авито перевёл поиск в другой раздел и по дороге выбросил радиус — повторяем уже в этом разделе.
            paths.insert(query.to_string(), page.path.clone());
            page = open(&tab, search_url(&page.path, query, Some(area))?.as_str()).await?;
            if !page.has_radius(area) {
                return Err(anyhow!(
                    "Авито не принял поиск в радиусе {} км от дома — проверьте город в «Настройках»",
                    area.radius_km
                )
                .into());
            }
        }
        page.into_offers()
    }
}

impl Page {
    fn has_radius(&self, area: Area) -> bool {
        self.radius.as_deref() == Some(area.radius_km.to_string().as_str())
    }

    /// Объявления на странице есть, а прочитать не удалось ни одного — значит, Авито поменял устройство карточек,
    /// и это должна быть видимая ошибка, а не тихое «ничего не нашлось».
    fn into_offers(self) -> Result<Vec<Offer>, SourceError> {
        let cards = self.cards.len();
        let offers: Vec<Offer> = self.cards.into_iter().filter_map(Card::into_offer).collect();
        if cards > 0 && offers.is_empty() {
            return Err(anyhow!(
                "Авито поменял устройство карточек: на странице {cards} объявлений, но ни одно не прочиталось"
            )
            .into());
        }
        // Забронированное уже не купить: продавец придержал его для другого покупателя.
        Ok(offers
            .into_iter()
            .filter(|offer| {
                !offer
                    .description
                    .as_deref()
                    .is_some_and(|text| text.contains("Забронировано"))
            })
            .collect())
    }
}

fn search_url(path: &str, query: &str, area: Option<Area>) -> anyhow::Result<Url> {
    let mut url = Url::parse(HOME)?.join(path)?;
    url.query_pairs_mut()
        .append_pair("q", query)
        .append_pair("s", NEWEST_FIRST);
    if let Some(area) = area {
        let radius = area.radius_km.to_string();
        url.query_pairs_mut()
            .append_pair("geoCoords", &format!("{},{}", area.center.lat, area.center.lon))
            .append_pair("radius", &radius)
            .append_pair("searchRadius", &radius);
    }
    Ok(url)
}

/// Переходит по адресу и ждёт, пока новая страница покажет объявления, «ничего не найдено», проверку или 404.
async fn open(tab: &Tab, url: &str) -> Result<Page, SourceError> {
    page::open(tab, url, READ_SCRIPT, NOT_FOUND).await
}

impl ScriptPage for Page {
    fn stale(&self) -> bool {
        self.stale
    }

    fn state(&self) -> &PageState {
        &self.state
    }
}

impl Card {
    fn into_offer(self) -> Option<Offer> {
        let price = self.price?.trim().parse::<u64>().ok().filter(|&price| price > 0)?;
        let (distance_km, place) = self.place.as_deref().map_or((None, None), split_distance);
        Some(Offer {
            name: self.title,
            price,
            old_price: None,
            rating: self.seller_rating.as_deref().and_then(parse_rating),
            reviews: self.seller_reviews.as_deref().and_then(leading_number),
            url: self.url,
            place,
            distance_km,
            posted: self.posted,
            description: self.text,
            delivery: None,
            sold_by_ozon: false,
            brand_verified: false,
        })
    }
}

/// «2,4 км · Валовая ул. Павелецкая» → 2.4 и адрес; «500 м · …» → 0.5. Без радиуса Авито расстояние не пишет.
fn split_distance(text: &str) -> (Option<f64>, Option<String>) {
    static DISTANCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d+(?:,\d+)?)\s*(км|м)\s*·\s*(.+)$").unwrap());
    let Some(found) = DISTANCE.captures(text) else {
        return (None, Some(text.to_string()));
    };
    let value: Option<f64> = found[1].replace(',', ".").parse().ok();
    let km = value.map(|value| if &found[2] == "м" { value / 1000.0 } else { value });
    (km, Some(found[3].to_string()))
}

fn parse_rating(text: &str) -> Option<f32> {
    text.trim()
        .replace(',', ".")
        .parse()
        .ok()
        .filter(|rating| (0.0..=5.0).contains(rating))
}

/// «2 453 отзыва» → 2453.
fn leading_number(text: &str) -> Option<u64> {
    text.chars()
        .take_while(|c| c.is_ascii_digit() || c.is_whitespace())
        .filter(char::is_ascii_digit)
        .collect::<String>()
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geo::GeoPoint;

    fn card(price: Option<&str>, place: Option<&str>) -> Card {
        Card {
            title: "Шуруповерт makita 8280dwae".into(),
            url: "https://www.avito.ru/moskva/instrumenty/shurupovert_1".into(),
            price: price.map(Into::into),
            place: place.map(Into::into),
            posted: Some("1 час назад".into()),
            seller_rating: Some("4,5".into()),
            seller_reviews: Some("2 453 отзыва".into()),
            text: Some("В отличном состоянии, в комплекте кейс".into()),
        }
    }

    #[test]
    fn distance_and_place_are_split() {
        assert_eq!(
            split_distance("2,4 км · Валовая ул. Павелецкая"),
            (Some(2.4), Some("Валовая ул. Павелецкая".into()))
        );
        assert_eq!(
            split_distance("500 м · Пр. Воскресенские Ворота Охотный ряд"),
            (Some(0.5), Some("Пр. Воскресенские Ворота Охотный ряд".into()))
        );
        assert_eq!(
            split_distance("Люблино, до 5 мин."),
            (None, Some("Люблино, до 5 мин.".into()))
        );
    }

    #[test]
    fn card_becomes_offer_with_seller_rating() {
        let offer = card(Some("3222"), Some("13 км · Краснодарская ул., 57к2 Люблино"))
            .into_offer()
            .unwrap();
        assert_eq!(offer.price, 3222);
        assert_eq!(offer.distance_km, Some(13.0));
        assert_eq!(offer.place.as_deref(), Some("Краснодарская ул., 57к2 Люблино"));
        assert_eq!(offer.rating, Some(4.5));
        assert_eq!(offer.reviews, Some(2453));
        assert_eq!(offer.posted.as_deref(), Some("1 час назад"));
    }

    #[test]
    fn card_without_price_is_skipped() {
        assert!(card(None, None).into_offer().is_none());
        assert!(card(Some("0"), None).into_offer().is_none());
        assert!(card(Some("договорная"), None).into_offer().is_none());
    }

    fn page(radius: Option<&str>, cards: Vec<Card>) -> Page {
        Page {
            stale: false,
            state: PageState::Listings,
            path: "/moskva/instrumenty".into(),
            radius: radius.map(Into::into),
            cards,
        }
    }

    #[test]
    fn dropped_radius_is_noticed() {
        let near = Area {
            center: GeoPoint { lat: 55.75, lon: 37.62 },
            radius_km: 25,
        };
        assert!(page(Some("25"), Vec::new()).has_radius(near));
        assert!(!page(None, Vec::new()).has_radius(near));
        assert!(!page(Some("5"), Vec::new()).has_radius(near));
    }

    #[test]
    fn unreadable_cards_are_an_error_but_empty_page_is_not() {
        assert!(page(None, Vec::new()).into_offers().unwrap().is_empty());
        assert_eq!(
            page(None, vec![card(Some("3510"), None)]).into_offers().unwrap().len(),
            1
        );
        let broken = page(None, vec![card(None, None), card(None, None)]).into_offers();
        assert!(
            matches!(broken, Err(SourceError::Failed(error)) if error.to_string().contains("поменял устройство карточек"))
        );
    }

    #[test]
    fn reserved_listings_are_skipped() {
        let reserved = Card {
            text: Some("Забронировано Наушники logitech g435 1 300 ₽".into()),
            ..card(Some("1300"), None)
        };
        let offers = page(None, vec![reserved, card(Some("3510"), None)])
            .into_offers()
            .unwrap();
        assert_eq!(offers.iter().map(|offer| offer.price).collect::<Vec<_>>(), [3510]);
    }

    #[test]
    fn search_url_carries_date_sort_and_radius() {
        let path = "/moskva/remont_i_stroitelstvo/instrumenty/elektroinstrument/dreli_i_shurupovyorty-ASgB";
        let near = Area {
            center: GeoPoint {
                lat: 55.751_244,
                lon: 37.618_423,
            },
            radius_km: 25,
        };
        assert_eq!(
            search_url(path, "шуруповерт makita", Some(near)).unwrap().as_str(),
            "https://www.avito.ru/moskva/remont_i_stroitelstvo/instrumenty/elektroinstrument/dreli_i_shurupovyorty-ASgB\
             ?q=%D1%88%D1%83%D1%80%D1%83%D0%BF%D0%BE%D0%B2%D0%B5%D1%80%D1%82+makita&s=104\
             &geoCoords=55.751244%2C37.618423&radius=25&searchRadius=25"
        );
        assert!(!search_url(path, "дрель", None).unwrap().as_str().contains("geoCoords"));
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Авито: cargo test -- --ignored --test-threads=1"]
    async fn finds_listings_near_point_on_live_site() {
        let avito = Avito::new(crate::browser::test_browser(), "moskva");
        let near = Area {
            center: GeoPoint {
                lat: 55.751_244,
                lon: 37.618_423,
            },
            radius_km: 5,
        };

        let offers = avito.search("шуруповерт makita", Some(near)).await.unwrap();

        assert!(offers.len() >= 10, "собрано всего {}", offers.len());
        assert!(
            offers
                .iter()
                .all(|o| o.price > 0 && o.url.starts_with("https://www.avito.ru/"))
        );
        assert!(
            offers.iter().all(|o| o.distance_km.is_some_and(|km| km <= 5.5)),
            "{offers:#?}"
        );
        println!(
            "собрано {}, первое: {} ₽ {} — {:?} км, {:?}",
            offers.len(),
            offers[0].price,
            offers[0].name,
            offers[0].distance_km,
            offers[0].place
        );
    }
}
