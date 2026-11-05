use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use regex::Regex;
use reqwest::Url;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::browser::Browser;
use crate::marketplace::page::{self, Listing, PageState, Paging, ScriptPage, rating, rubles};
use crate::marketplace::{OfferSource, Product, Scope, SourceError};
use crate::offer::Offer;

const HOME: &str = "https://www.dns-shop.ru/";
const READ_SCRIPT: &str = include_str!("dns/read.js");
const PRODUCT_SCRIPT: &str = include_str!("dns/product.js");
const PRODUCT_NOT_FOUND: &str =
    "ДНС не нашёл такой товар — откройте ссылку в браузере: может быть, его сняли с продажи";
const NOT_FOUND: &str = "ДНС открыл не выдачу, а страницу бренда — уточните запрос: не «makita», а «шуруповерт makita»";
/// Страница ДНС — 24 товара, для рынка мало. Три страницы — 72, а переходов на сайт за проверку всего три.
/// Между страницами — пауза, как у человека, листающего выдачу: защита ДНС строже, чем у Ozon.
const PAGING: Paging = Paging {
    pages: 3,
    number_key: "p",
    pause: Duration::from_secs(3),
    full_page: 24,
};

/// Магазин ДНС. Поиск он сам переводит в каталог нужной категории («шуруповерт makita» → «Шуруповерты»),
/// а цены дорисовывает после загрузки страницы — поэтому, как и Авито, вкладка открывает поиск и читает готовые карточки.
/// Цены и наличие зависят от города, выбранного на сайте в окне браузера программы.
pub struct Dns {
    browser: Arc<Browser>,
    tab_in_use: Mutex<()>,
}

#[derive(Deserialize)]
struct Page {
    stale: bool,
    state: PageState,
    url: String,
    city: Option<String>,
    cards: Vec<Card>,
}

#[derive(Deserialize)]
struct Card {
    title: String,
    url: String,
    in_stock: bool,
    price: Option<String>,
    old_price: Option<String>,
    rating: Option<String>,
    reviews: Option<String>,
    place: Option<String>,
    delivery: Option<String>,
}

impl Dns {
    pub fn new(browser: Arc<Browser>) -> Self {
        Self {
            browser,
            tab_in_use: Mutex::new(()),
        }
    }
}

#[async_trait]
impl OfferSource for Dns {
    fn name(&self) -> &'static str {
        "ДНС"
    }

    /// Адресов у товаров магазина нет: он везёт со склада или из своих магазинов, поэтому `area` не учитывается.
    async fn search(&self, query: &str, _scope: Scope) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.browser.tab(HOME).await?;
        let url = Url::parse_with_params(&format!("{HOME}search/"), &[("q", query)])
            .context("не удалось составить адрес поиска")?;
        page::read_pages::<Page>(&tab, url.as_str(), READ_SCRIPT, NOT_FOUND, &PAGING).await
    }

    /// Фильтры ДНС хранит в самом адресе каталога (бренд — частью пути подборки), поэтому ссылка открывается как есть.
    async fn search_link(&self, link: &Url, _scope: Scope) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.browser.tab(HOME).await?;
        page::read_pages::<Page>(&tab, link.as_str(), READ_SCRIPT, NOT_FOUND, &PAGING).await
    }

    async fn product(&self, link: &Url) -> Result<Product, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.browser.tab(HOME).await?;
        let page: ProductPage = page::open(&tab, link.as_str(), PRODUCT_SCRIPT, PRODUCT_NOT_FOUND).await?;
        if let Some(city) = &page.city {
            log::info!("ДНС: {link}, город на сайте — {city}");
        }
        Ok(page.into_product())
    }
}

impl ScriptPage for Page {
    fn stale(&self) -> bool {
        self.stale
    }

    fn state(&self) -> &PageState {
        &self.state
    }
}

/// Страница одного товара, прочитанная `dns/product.js`.
#[derive(Deserialize)]
struct ProductPage {
    stale: bool,
    state: PageState,
    name: String,
    price: Option<u64>,
    in_stock: bool,
    city: Option<String>,
}

impl ScriptPage for ProductPage {
    fn stale(&self) -> bool {
        self.stale
    }

    fn state(&self) -> &PageState {
        &self.state
    }
}

impl ProductPage {
    /// Не в наличии — цены нет, даже если ДНС её пишет: купить по ней всё равно нельзя.
    fn into_product(self) -> Product {
        Product {
            name: self.name,
            price: self.price.filter(|_| self.in_stock),
            price_without_card: None,
        }
    }
}

impl Listing for Page {
    fn url(&self) -> &str {
        &self.url
    }

    fn cards(&self) -> usize {
        self.cards.len()
    }

    fn note(&self) -> Option<String> {
        self.city
            .as_ref()
            .map(|city| format!("ДНС: {}, город на сайте — {city}", self.url))
    }

    /// Товары в наличии на странице есть, а прочитать не удалось ни одного — ДНС поменял устройство карточек,
    /// и это должна быть видимая ошибка, а не тихое «ничего не нашлось». Товары не в наличии пропускаются.
    fn into_offers(self) -> Result<Vec<Offer>, SourceError> {
        let in_stock = self.cards.iter().filter(|card| card.in_stock).count();
        let offers: Vec<Offer> = self.cards.into_iter().filter_map(Card::into_offer).collect();
        if in_stock > 0 && offers.is_empty() {
            return Err(anyhow!(
                "ДНС поменял устройство карточек: в наличии {in_stock} товаров, но ни один не прочитался"
            )
            .into());
        }
        Ok(offers)
    }
}

impl Card {
    fn into_offer(self) -> Option<Offer> {
        if !self.in_stock {
            return None;
        }
        let price = self.price.as_deref().and_then(rubles)?;
        Some(Offer {
            name: self.title,
            price,
            old_price: self.old_price.as_deref().and_then(rubles).filter(|&old| old > price),
            rating: self.rating.as_deref().and_then(rating),
            reviews: self.reviews.as_deref().and_then(reviews),
            url: self.url,
            place: self.place,
            distance_km: None,
            posted: None,
            description: None,
            delivery: self.delivery.as_deref().and_then(delivery),
            sold_by_ozon: false,
            brand_verified: false,
        })
    }
}

static REVIEWS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d+(?:[.,]\d+)?)\s*([kк])?").unwrap());

/// «4.53 | 4.9k отзывов» → 4900: после черты — число отзывов, тысячи ДНС сокращает буквой k.
fn reviews(text: &str) -> Option<u64> {
    let count = text.split_once('|').map_or(text, |(_, count)| count);
    let found = REVIEWS.captures(count)?;
    let value: f64 = found[1].replace(',', ".").parse().ok()?;
    let thousands = if found.get(2).is_some() { 1000.0 } else { 1.0 };
    // Отзывов меньше 2⁵³ и не бывает отрицательных — перевод в целое точен.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some((value * thousands).round() as u64)
}

/// «Доставим на дом завтра» → «Завтра»: срок в том же виде, что у Ozon, чтобы работало «доставка завтра».
fn delivery(text: &str) -> Option<String> {
    let when = text
        .trim()
        .trim_start_matches("Доставим на дом")
        .trim_start_matches("Доставка")
        .trim();
    let mut letters = when.chars();
    let first = letters.next()?;
    Some(first.to_uppercase().chain(letters).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(price: Option<&str>, in_stock: bool) -> Card {
        Card {
            title: "Беспроводные наушники Logitech G435 черный 2021".into(),
            url: "https://www.dns-shop.ru/product/521e6a67f5b2ed20/besprovodnye-nausniki-logitech-g435-cernyj-2021/"
                .into(),
            in_stock,
            price: price.map(Into::into),
            old_price: None,
            rating: Some("4.53".into()),
            reviews: Some("4.53 | 4.9k отзывов".into()),
            place: Some("В наличии в 106 магазинах".into()),
            delivery: Some("Доставим на дом завтра".into()),
        }
    }

    fn page(cards: Vec<Card>) -> Page {
        Page {
            stale: false,
            state: PageState::Listings,
            url: "https://www.dns-shop.ru/catalog/17a9c6ec16404e77/surupoverty-dreli-surupoverty/?q=makita".into(),
            city: Some("Москва".into()),
            cards,
        }
    }

    #[test]
    fn page_names_its_city_for_the_journal() {
        let page = page(Vec::new());
        assert_eq!(
            page.note().as_deref(),
            Some(
                "ДНС: https://www.dns-shop.ru/catalog/17a9c6ec16404e77/surupoverty-dreli-surupoverty/?q=makita, \
                 город на сайте — Москва"
            )
        );
        assert_eq!(page.cards(), 0);
    }

    #[test]
    fn card_becomes_offer_with_delivery_in_ozon_words() {
        let offer = card(Some("6 399 ₽"), true).into_offer().unwrap();

        assert_eq!(
            (offer.price, offer.rating, offer.reviews),
            (6_399, Some(4.53), Some(4_900))
        );
        assert_eq!(offer.delivery.as_deref(), Some("Завтра"));
        assert!(offer.arrives_by_tomorrow());
        assert_eq!(offer.place.as_deref(), Some("В наличии в 106 магазинах"));
    }

    #[test]
    fn discount_is_kept_only_when_old_price_is_higher() {
        let discounted = Card {
            old_price: Some("7 999".into()),
            ..card(Some("6 399 ₽"), true)
        };
        assert_eq!(discounted.into_offer().unwrap().old_price, Some(7_999));
    }

    #[test]
    fn reviews_in_thousands_and_plain_numbers() {
        assert_eq!(reviews("4.53 | 4.9k отзывов"), Some(4_900));
        assert_eq!(reviews("4.21 | 1.6к отзывов"), Some(1_600));
        assert_eq!(reviews("4.80 | 37 отзывов"), Some(37));
        assert_eq!(reviews("4.80 |"), None);
    }

    #[test]
    fn delivery_date_is_kept_as_written() {
        assert_eq!(delivery("Доставим на дом 15 октября").as_deref(), Some("15 октября"));
        assert_eq!(delivery("Доставим на дом").as_deref(), None);
    }

    #[test]
    fn out_of_stock_is_skipped_but_unreadable_stock_is_an_error() {
        let offers = page(vec![card(None, false), card(Some("6 399 ₽"), true)])
            .into_offers()
            .unwrap();
        assert_eq!(offers.len(), 1);

        assert!(page(vec![card(None, false)]).into_offers().unwrap().is_empty());

        let broken = page(vec![card(None, true), card(Some("—"), true)]).into_offers();
        assert!(
            matches!(broken, Err(SourceError::Failed(error)) if error.to_string().contains("поменял устройство карточек"))
        );
    }

    /// Так `dns/product.js` отдаёт страницу наушников 29.10.2026 — из описания товара schema.org.
    #[test]
    fn product_page_has_price_only_when_in_stock() {
        let read = |in_stock| -> Product {
            serde_json::from_value::<ProductPage>(serde_json::json!({
                "stale": false, "state": "listings", "city": "Москва", "in_stock": in_stock,
                "name": "Беспроводные наушники Sony WH-CH520 черный 2023", "price": 4299
            }))
            .unwrap()
            .into_product()
        };
        assert_eq!(read(true).price, Some(4_299));
        assert_eq!(read(false).price, None);
        assert_eq!(read(true).price_without_card, None, "цен с картой и без у ДНС нет");
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::browser::test_browser;

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой ДНС: cargo test -- --ignored --test-threads=1"]
    async fn finds_offers_on_live_site() {
        let dns = Dns::new(test_browser());

        let offers = dns.search("шуруповерт makita", Scope::default()).await.unwrap();

        assert!(
            offers.len() > 24,
            "собрано всего {} — читается только первая страница из 24 товаров",
            offers.len()
        );
        assert!(offers.iter().all(|offer| !offer.name.is_empty()
            && offer.price > 0
            && offer.url.starts_with("https://www.dns-shop.ru/product/")));
        assert!(
            offers
                .iter()
                .any(|offer| offer.rating.is_some() && offer.reviews.is_some() && offer.delivery.is_some())
        );
        println!(
            "собрано {}, первое: {} ₽ {} ({:?})",
            offers.len(),
            offers[0].price,
            offers[0].name,
            offers[0].delivery
        );
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой ДНС: cargo test -- --ignored --test-threads=1"]
    async fn reads_product_page_on_live_site() {
        let dns = Dns::new(test_browser());
        let link = crate::marketplace::SourceKind::Dns
            .parse_product_link(
                "https://www.dns-shop.ru/product/94161cf40137d9cb/besprovodnye-nausniki-sony-wh-ch520-cernyj-2023/",
            )
            .unwrap();

        let product = dns.product(&link).await.unwrap();

        assert!(product.name.contains("WH-CH520"), "{product:?}");
        assert!(product.price.is_some(), "пропала цена из описания товара: {product:?}");
        println!("{product:?}");
    }

    /// Подборка «наушники Sony» — в неё ДНС сам переводит такой поиск; бренд в ней — часть пути.
    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой ДНС: cargo test -- --ignored --test-threads=1"]
    async fn finds_offers_by_catalog_link_on_live_site() {
        let dns = Dns::new(test_browser());
        let link = crate::marketplace::SourceKind::Dns
            .parse_link("https://www.dns-shop.ru/catalog/recipe/422f8a94ba41d693/sony/")
            .unwrap();

        let offers = dns.search_link(&link, Scope::default()).await.unwrap();

        assert!(
            offers.len() > 24,
            "собрано всего {} — следующие страницы подборки не прочитались",
            offers.len()
        );
        // Гарнитуры PlayStation выпускает Sony, и ДНС держит их в подборке, хотя в названии слова «Sony» нет.
        let other_brands: Vec<&str> = offers
            .iter()
            .map(|offer| offer.name.as_str())
            .filter(|name| {
                !["sony", "playstation"]
                    .iter()
                    .any(|brand| name.to_lowercase().contains(brand))
            })
            .collect();
        assert!(
            other_brands.is_empty(),
            "фильтр подборки потерялся на следующих страницах: {other_brands:?}"
        );
    }
}
