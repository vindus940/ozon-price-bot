use std::sync::{Arc, LazyLock};

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use regex::Regex;
use reqwest::Url;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::browser::Browser;
use crate::marketplace::page::{self, PageState, ScriptPage};
use crate::marketplace::{Area, OfferSource, SourceError};
use crate::offer::Offer;

const HOME: &str = "https://www.dns-shop.ru/";
const READ_SCRIPT: &str = include_str!("dns/read.js");
const NOT_FOUND: &str = "ДНС открыл не выдачу, а страницу бренда — уточните запрос: не «makita», а «шуруповерт makita»";

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
    async fn search(&self, query: &str, _area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.browser.tab(HOME).await?;
        let url = Url::parse_with_params(&format!("{HOME}search/"), &[("q", query)])
            .context("не удалось составить адрес поиска")?;
        let page: Page = page::open(&tab, url.as_str(), READ_SCRIPT, NOT_FOUND).await?;
        if let Some(city) = &page.city {
            log::info!("ДНС: «{query}», город на сайте — {city}");
        }
        page.into_offers()
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

impl Page {
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

/// «6 399 ₽» → 6399.
fn rubles(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|&rubles| rubles > 0)
}

fn rating(text: &str) -> Option<f32> {
    text.trim().replace(',', ".").parse().ok()
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
            city: Some("Москва".into()),
            cards,
        }
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
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::browser::test_browser;

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой ДНС: cargo test -- --ignored --test-threads=1"]
    async fn finds_offers_on_live_site() {
        let dns = Dns::new(test_browser());

        let offers = dns.search("шуруповерт makita", None).await.unwrap();

        assert!(offers.len() >= 10, "собрано всего {}", offers.len());
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
}
