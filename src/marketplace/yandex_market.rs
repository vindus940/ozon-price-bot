use std::sync::{Arc, LazyLock};
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use regex::Regex;
use reqwest::Url;
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::browser::Browser;
use crate::marketplace::page::{self, Listing, PageState, Paging, ScriptPage, count, rating, rubles};
use crate::marketplace::{OfferSource, Product, Scope, SourceError};
use crate::offer::Offer;

const HOME: &str = "https://market.yandex.ru/";
const READ_SCRIPT: &str = include_str!("yandex_market/read.js");
const NOT_FOUND: &str = "Яндекс Маркет не открыл выдачу — проверьте запрос или ссылку";
/// Третий переход за пару минут Маркет встретил проверкой «я не робот» (5.11.2026), поэтому страниц две,
/// а пауза между ними длиннее, чем у ДНС.
const PAGING: Paging = Paging {
    pages: 2,
    number_key: "page",
    pause: Duration::from_secs(5),
    full_page: 8,
};

/// Яндекс Маркет. Как у ДНС, вкладка открывает поиск и читает готовые карточки, а страницы листаются адресом:
/// `page=2` приносит другие товары и в скрытой вкладке. Цена — как на плитке, «с картой Яндекс Пэй».
pub struct YandexMarket {
    browser: Arc<Browser>,
    tab_in_use: Mutex<()>,
}

#[derive(Deserialize)]
struct Page {
    stale: bool,
    state: PageState,
    url: String,
    cards: Vec<Card>,
}

#[derive(Deserialize)]
struct Card {
    url: String,
    name: Option<String>,
    price: Option<String>,
    reviews: Option<String>,
    delivery: Option<String>,
}

impl YandexMarket {
    pub fn new(browser: Arc<Browser>) -> Self {
        Self {
            browser,
            tab_in_use: Mutex::new(()),
        }
    }
}

#[async_trait]
impl OfferSource for YandexMarket {
    fn name(&self) -> &'static str {
        "Яндекс Маркет"
    }

    /// Адресов у товаров нет: Маркет везёт со складов, поэтому область не учитывается.
    async fn search(&self, query: &str, _scope: Scope) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.browser.tab(HOME).await?;
        let url = Url::parse_with_params(&format!("{HOME}search"), &[("text", query)])
            .context("не удалось составить адрес поиска")?;
        page::read_pages::<Page>(&tab, url.as_str(), READ_SCRIPT, NOT_FOUND, &PAGING).await
    }

    /// Фильтры Маркет хранит в адресе выдачи, поэтому ссылка открывается как есть.
    async fn search_link(&self, link: &Url, _scope: Scope) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.browser.tab(HOME).await?;
        page::read_pages::<Page>(&tab, link.as_str(), READ_SCRIPT, NOT_FOUND, &PAGING).await
    }

    /// Страницу товара Маркета ещё не разведали — форма такую слежку не даст создать (`SourceKind::has_product_pages`).
    async fn product(&self, _link: &Url) -> Result<Product, SourceError> {
        Err(anyhow!("Яндекс Маркет: за ценой одного товара программа пока не следит — только Ozon и ДНС").into())
    }

    async fn search_cheapest(&self, query: &str, from: u64, to: u64) -> Result<Vec<Offer>, SourceError> {
        self.search_link(&cheapest_url(query, from, to)?, Scope::default())
            .await
    }
}

/// `how=aprice` — от дешёвых, `pricefrom` и `priceto` — коридор цен (5.11.2026).
fn cheapest_url(query: &str, from: u64, to: u64) -> anyhow::Result<Url> {
    Url::parse_with_params(
        &format!("{HOME}search"),
        &[
            ("text", query),
            ("how", "aprice"),
            ("pricefrom", &from.to_string()),
            ("priceto", &to.to_string()),
        ],
    )
    .context("не удалось составить адрес поиска")
}

impl ScriptPage for Page {
    fn stale(&self) -> bool {
        self.stale
    }

    fn state(&self) -> &PageState {
        &self.state
    }
}

impl Listing for Page {
    fn url(&self) -> &str {
        &self.url
    }

    fn cards(&self) -> usize {
        self.cards.len()
    }

    /// Карточки на странице есть, а прочитать не удалось ни одной — Маркет поменял разметку, и это должна быть видимая
    /// ошибка, а не тихое «ничего не нашлось».
    fn into_offers(self) -> Result<Vec<Offer>, SourceError> {
        let cards = self.cards.len();
        let offers: Vec<Offer> = self.cards.into_iter().filter_map(Card::into_offer).collect();
        if cards > 0 && offers.is_empty() {
            return Err(anyhow!(
                "Яндекс Маркет поменял устройство карточек: на странице {cards}, но ни одна не прочиталась"
            )
            .into());
        }
        Ok(offers)
    }
}

impl Card {
    fn into_offer(self) -> Option<Offer> {
        let price = self.price.as_deref().and_then(rubles)?;
        let (rating, reviews) = self.reviews.as_deref().map_or((None, None), rating_and_reviews);
        Some(Offer {
            name: self.name?,
            price,
            old_price: None,
            rating,
            reviews,
            url: self.url,
            place: None,
            distance_km: None,
            posted: None,
            description: None,
            delivery: self.delivery.as_deref().and_then(delivery_term),
            sold_by_ozon: false,
            brand_verified: false,
        })
    }
}

static RATING_AND_REVIEWS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^(\d(?:[.,]\d)?)\s*\(([\d\s]+)\)").unwrap());

/// «4.9 (25) · 26 купили» → 4.9 и 25 оценок; без скобок число купивших за оценки не считается.
fn rating_and_reviews(text: &str) -> (Option<f32>, Option<u64>) {
    RATING_AND_REVIEWS
        .captures(text.trim())
        .map_or((None, None), |found| (rating(&found[1]), count(&found[2])))
}

/// «Завтра , ПВЗ По клику» → «Завтра»: до запятой срок, после — способ получения.
fn delivery_term(text: &str) -> Option<String> {
    let term = text.split(',').next()?.trim();
    (!term.is_empty()).then(|| term.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Так `read.js` отдаёт первую карточку выдачи «наушники sony» 5.11.2026.
    fn card() -> Card {
        Card {
            url: "https://market.yandex.ru/card/naushniki-sony-wh-ch520/5547272884".into(),
            name: Some("Наушники Sony WH-CH520 , полноразмерные, беспроводные".into()),
            price: Some("4\u{a0}220\u{a0}₽".into()),
            reviews: Some("4.9 (25) · 26 купили".into()),
            delivery: Some("Завтра , ПВЗ По клику".into()),
        }
    }

    fn page(cards: Vec<Card>) -> Page {
        Page {
            stale: false,
            state: PageState::Listings,
            url: "https://market.yandex.ru/search?text=sony".into(),
            cards,
        }
    }

    #[test]
    fn cheapest_first_sorts_by_price_within_the_corridor() {
        assert_eq!(
            cheapest_url("sony", 2_000, 4_000).unwrap().as_str(),
            "https://market.yandex.ru/search?text=sony&how=aprice&pricefrom=2000&priceto=4000"
        );
    }

    #[test]
    fn card_becomes_offer_with_rating_reviews_and_delivery_term() {
        assert_eq!(
            card().into_offer(),
            Some(Offer {
                rating: Some(4.9),
                reviews: Some(25),
                url: "https://market.yandex.ru/card/naushniki-sony-wh-ch520/5547272884".into(),
                delivery: Some("Завтра".into()),
                ..Offer::sample("Наушники Sony WH-CH520 , полноразмерные, беспроводные", 4_220)
            })
        );
    }

    #[test]
    fn delivery_term_is_what_the_by_tomorrow_filter_understands() {
        let arrives = |text: &str| {
            let offer = Offer {
                delivery: delivery_term(text),
                ..Offer::sample("Наушники", 1_000)
            };
            offer.arrives_by_tomorrow()
        };
        assert!(arrives("Завтра , ПВЗ По клику"));
        assert!(arrives("За 3 часа , курьер По клику"));
        assert!(!arrives("Послезавтра , по клику"));
        assert_eq!(delivery_term(" , курьер"), None);
    }

    #[test]
    fn bought_count_is_not_taken_for_reviews() {
        assert_eq!(rating_and_reviews("4.5 (1 043) · 207 купили"), (Some(4.5), Some(1_043)));
        assert_eq!(rating_and_reviews("5 (3)"), (Some(5.0), Some(3)));
        assert_eq!(rating_and_reviews("26 купили"), (None, None));
    }

    #[test]
    fn card_without_price_or_name_is_skipped_and_without_rating_stays() {
        assert_eq!(Card { price: None, ..card() }.into_offer(), None);
        assert_eq!(Card { name: None, ..card() }.into_offer(), None);
        let offer = Card {
            reviews: None,
            ..card()
        }
        .into_offer()
        .unwrap();
        assert_eq!((offer.rating, offer.reviews), (None, None));
    }

    #[test]
    fn unreadable_cards_are_an_error_but_an_empty_page_is_not() {
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
    #[ignore = "открывает браузер и ходит на живой Яндекс Маркет: cargo test -- --ignored --test-threads=1"]
    async fn finds_offers_on_live_site() {
        let market = YandexMarket::new(test_browser());

        let offers = market.search("наушники sony", Scope::default()).await.unwrap();

        assert!(
            offers.len() > 8,
            "собрано всего {} — вторая страница не прочиталась",
            offers.len()
        );
        assert!(offers.iter().all(|offer| !offer.name.is_empty()
            && offer.price > 0
            && offer.url.starts_with("https://market.yandex.ru/")
            && !offer.url.contains('?')));
        assert!(
            offers
                .iter()
                .any(|offer| offer.rating.is_some() && offer.reviews.is_some()),
            "пропали рейтинг и оценки"
        );
        assert!(
            offers.iter().any(|offer| offer.delivery.is_some()),
            "пропал срок доставки"
        );
        println!(
            "собрано {}, первое: {} ₽ {} — {}",
            offers.len(),
            offers[0].price,
            offers[0].name,
            offers[0].url
        );
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Яндекс Маркет: cargo test -- --ignored --test-threads=1"]
    async fn impossible_price_filter_is_empty_not_a_failure() {
        let market = YandexMarket::new(test_browser());
        let link = Url::parse("https://market.yandex.ru/search?text=sony&pricefrom=90000000").unwrap();

        let offers = market.search_link(&link, Scope::default()).await.unwrap();

        assert!(offers.is_empty(), "собрано {}", offers.len());
    }
}
