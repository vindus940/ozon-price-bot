use anyhow::{Context, anyhow};
use async_trait::async_trait;
use reqwest::Url;
use serde::Deserialize;
use std::sync::Arc;
use tokio::sync::Mutex;

use crate::browser::Browser;
use crate::marketplace::page::{self, PageState, ScriptPage, count, rating, rubles};
use crate::marketplace::{OfferSource, Product, Scope, SourceError};
use crate::offer::Offer;

const HOME: &str = "https://www.wildberries.ru/";
const READ_SCRIPT: &str = include_str!("wildberries/read.js");
const NOT_FOUND: &str = "Wildberries не открыл выдачу — проверьте запрос или ссылку";
/// Wildberries. Выдачу страница получает из внутреннего API, но тот же запрос из скрипта вкладки WB отклоняет (403,
/// 29.10.2026) — поэтому, как у ДНС, вкладка открывает поиск и читает готовые карточки. Читается первый экран,
/// 40 товаров: `page=2` WB не замечает, а следующие товары подгружает прокруткой только в видимом окне (5.11.2026),
/// и рынок не должен зависеть от того, свёрнута ли программа.
/// Цены — как на сайте, «с WB Кошельком»; город доставки — из профиля браузера программы.
pub struct Wildberries {
    browser: Arc<Browser>,
    tab_in_use: Mutex<()>,
}

#[derive(Deserialize)]
struct Page {
    stale: bool,
    state: PageState,
    cards: Vec<Card>,
}

#[derive(Deserialize)]
struct Card {
    id: String,
    brand: Option<String>,
    name: Option<String>,
    price: Option<String>,
    old_price: Option<String>,
    rating: Option<String>,
    reviews: Option<String>,
    delivery: Option<String>,
}

impl Wildberries {
    pub fn new(browser: Arc<Browser>) -> Self {
        Self {
            browser,
            tab_in_use: Mutex::new(()),
        }
    }
}

#[async_trait]
impl OfferSource for Wildberries {
    fn name(&self) -> &'static str {
        "Wildberries"
    }

    /// Адресов у товаров нет: WB везёт со склада, поэтому область не учитывается.
    async fn search(&self, query: &str, _scope: Scope) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.browser.tab(HOME).await?;
        let url = Url::parse_with_params(&format!("{HOME}catalog/0/search.aspx"), &[("search", query)])
            .context("не удалось составить адрес поиска")?;
        page::open::<Page>(&tab, url.as_str(), READ_SCRIPT, NOT_FOUND)
            .await?
            .into_offers()
    }

    /// Фильтры WB хранит в адресе выдачи, поэтому ссылка открывается как есть.
    async fn search_link(&self, link: &Url, _scope: Scope) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.browser.tab(HOME).await?;
        page::open::<Page>(&tab, link.as_str(), READ_SCRIPT, NOT_FOUND)
            .await?
            .into_offers()
    }

    /// Страницу товара WB ещё не разведали — форма такую слежку не даст создать (`SourceKind::has_product_pages`).
    async fn product(&self, _link: &Url) -> Result<Product, SourceError> {
        Err(anyhow!("Wildberries: за ценой одного товара программа пока не следит — только Ozon и ДНС").into())
    }

    async fn search_cheapest(&self, query: &str, from: u64, to: u64) -> Result<Vec<Offer>, SourceError> {
        self.search_link(&cheapest_url(query, from, to)?, Scope::default())
            .await
    }
}

/// `sort=priceup` — от дешёвых, `priceU` — коридор в копейках через точку с запятой, не закодированной (5.11.2026).
fn cheapest_url(query: &str, from: u64, to: u64) -> anyhow::Result<Url> {
    let mut url = Url::parse_with_params(
        &format!("{HOME}catalog/0/search.aspx"),
        &[("search", query), ("sort", "priceup")],
    )
    .context("не удалось составить адрес поиска")?;
    let params = format!("{}&priceU={}00;{}00", url.query().unwrap_or_default(), from, to);
    url.set_query(Some(&params));
    Ok(url)
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
    /// Карточки на странице есть, а прочитать не удалось ни одной — WB поменял разметку, и это должна быть видимая
    /// ошибка, а не тихое «ничего не нашлось».
    fn into_offers(self) -> Result<Vec<Offer>, SourceError> {
        let cards = self.cards.len();
        let offers: Vec<Offer> = self.cards.into_iter().filter_map(Card::into_offer).collect();
        if cards > 0 && offers.is_empty() {
            return Err(anyhow!(
                "Wildberries поменял устройство карточек: на странице {cards}, но ни одна не прочиталась"
            )
            .into());
        }
        Ok(offers)
    }
}

impl Card {
    fn into_offer(self) -> Option<Offer> {
        let price = self.price.as_deref().and_then(rubles)?;
        let name = full_name(self.brand.as_deref(), self.name.as_deref()?);
        Some(Offer {
            name,
            price,
            old_price: self.old_price.as_deref().and_then(rubles).filter(|&old| old > price),
            rating: self.rating.as_deref().and_then(rating),
            reviews: self.reviews.as_deref().and_then(count),
            url: format!("{HOME}catalog/{}/detail.aspx", self.id),
            place: None,
            distance_km: None,
            posted: None,
            description: None,
            delivery: self.delivery.filter(|when| !when.is_empty()),
            sold_by_ozon: false,
            brand_verified: false,
        })
    }
}

/// Бренд WB пишет отдельно от названия («SONY / Беспроводные наушники WH-CH520»), а слова запроса «sony» должны
/// найтись в названии — поэтому бренд ставится впереди, если в названии его ещё нет.
fn full_name(brand: Option<&str>, name: &str) -> String {
    match brand.map(str::trim).filter(|brand| !brand.is_empty()) {
        Some(brand) if !name.to_lowercase().contains(&brand.to_lowercase()) => format!("{brand} {name}"),
        _ => name.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Так `read.js` отдаёт первую карточку выдачи «наушники sony wh-ch520» 29.10.2026.
    fn card() -> Card {
        Card {
            id: "157950167".into(),
            brand: Some("SONY".into()),
            name: Some("Беспроводные наушники WH-CH520 черные".into()),
            price: Some("4\u{a0}394\u{a0}₽".into()),
            old_price: Some("8\u{a0}990\u{a0}₽".into()),
            rating: Some("4,9".into()),
            reviews: Some("· 196 оценок".into()),
            delivery: Some("1 ноября".into()),
        }
    }

    #[test]
    fn cheapest_first_sorts_by_price_within_the_corridor_in_kopecks() {
        assert_eq!(
            cheapest_url("sony wh", 2_000, 4_000).unwrap().as_str(),
            "https://www.wildberries.ru/catalog/0/search.aspx?search=sony+wh&sort=priceup&priceU=200000;400000"
        );
    }

    #[test]
    fn card_becomes_offer_with_brand_in_the_name() {
        assert_eq!(
            card().into_offer(),
            Some(Offer {
                old_price: Some(8_990),
                rating: Some(4.9),
                reviews: Some(196),
                url: "https://www.wildberries.ru/catalog/157950167/detail.aspx".into(),
                delivery: Some("1 ноября".into()),
                ..Offer::sample("SONY Беспроводные наушники WH-CH520 черные", 4_394)
            })
        );
    }

    #[test]
    fn brand_already_in_the_name_is_not_repeated_and_missing_parts_are_none() {
        assert_eq!(
            full_name(Some("Sony"), "Наушники Sony WH-1000XM5"),
            "Наушники Sony WH-1000XM5"
        );
        assert_eq!(full_name(None, "Наушники"), "Наушники");

        let new_item = Card {
            rating: None,
            reviews: Some("· 193\u{a0}208 оценок".into()),
            old_price: None,
            ..card()
        };
        let offer = new_item.into_offer().unwrap();
        assert_eq!(
            (offer.rating, offer.reviews, offer.old_price),
            (None, Some(193_208), None)
        );
    }

    #[test]
    fn unreadable_cards_are_an_error_but_nothing_found_is_not() {
        let page = |cards| Page {
            stale: false,
            state: PageState::Listings,
            cards,
        };
        assert!(page(Vec::new()).into_offers().unwrap().is_empty());
        let no_price = Card { price: None, ..card() };
        assert!(matches!(
            page(vec![no_price]).into_offers(),
            Err(SourceError::Failed(error)) if error.to_string().contains("поменял устройство карточек")
        ));
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use crate::browser::test_browser;

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Wildberries: cargo test -- --ignored --test-threads=1"]
    async fn finds_offers_on_live_site() {
        let wildberries = Wildberries::new(test_browser());

        let offers = wildberries.search("наушники sony", Scope::default()).await.unwrap();

        assert!(
            offers.len() >= 30,
            "собрано всего {} — первый экран выдачи WB — 40 товаров",
            offers.len()
        );
        assert!(offers.iter().all(|offer| !offer.name.is_empty()
            && offer.price > 0
            && offer.url.starts_with("https://www.wildberries.ru/catalog/")));
        assert!(
            offers
                .iter()
                .filter_map(|offer| offer.delivery.as_deref())
                .all(|when| !when.to_lowercase().contains("корзин")),
            "на кнопке вместо срока — надпись «В корзину»"
        );
        assert!(
            offers
                .iter()
                .any(|offer| offer.rating.is_some() && offer.reviews.is_some())
        );
        assert!(
            offers.iter().any(|offer| offer.delivery.is_some()),
            "пропал срок доставки"
        );
        println!(
            "собрано {}, первое: {} ₽ {}",
            offers.len(),
            offers[0].price,
            offers[0].name
        );
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Wildberries: cargo test -- --ignored --test-threads=1"]
    async fn nothing_found_is_empty_not_recommendations() {
        let wildberries = Wildberries::new(test_browser());

        let offers = wildberries.search("фывапролджэячс", Scope::default()).await.unwrap();

        assert!(
            offers.is_empty(),
            "рекомендации под «ничего не нашлось» попали в выдачу: {}",
            offers.len()
        );
    }
}
