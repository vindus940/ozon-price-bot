use std::time::Duration;

use anyhow::{Context, anyhow};
use reqwest::Url;
use serde::Deserialize;
use serde::de::DeserializeOwned;
use tokio::time::{Instant, sleep};

use crate::browser::Tab;
use crate::marketplace::SourceError;
use crate::offer::Offer;

const PAGE_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(700);
const MARK_PAGE_STALE: &str = "window.__huntStale = true";

/// Что скрипт площадки видит на открытой странице.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageState {
    Loading,
    Captcha,
    NotFound,
    /// Товары или «ничего не найдено» — выдача готова; у страницы одного товара — его описание с ценой.
    Listings,
}

/// Страница выдачи, прочитанная скриптом площадки.
pub trait ScriptPage: DeserializeOwned {
    /// Это ещё старая страница: программа помечает её перед переходом, а новая пометки не знает.
    fn stale(&self) -> bool;
    fn state(&self) -> &PageState;
}

/// Страница выдачи, которую листают: где она оказалась на самом деле и что на ней.
pub trait Listing: ScriptPage {
    /// Адрес открытой страницы: площадка могла перевести поиск в каталог, следующие страницы — от него.
    fn url(&self) -> &str;
    fn cards(&self) -> usize;
    /// Карточки, которые есть, но не прочитались, — ошибка, а не тихое «ничего не нашлось».
    fn into_offers(self) -> Result<Vec<Offer>, SourceError>;
    /// Что записать в журнал о первой странице — например, город, для которого площадка показала цены.
    fn note(&self) -> Option<String> {
        None
    }
}

/// Как листать выдачу площадки: сколько страниц, каким параметром адреса номер и сколько ждать между ними.
pub struct Paging {
    pub pages: u32,
    pub number_key: &'static str,
    pub pause: Duration,
}

/// Некоторые площадки (Авито, ДНС, Wildberries, Яндекс Маркет) отдают выдачу только при настоящем переходе: запрос из скрипта
/// получает пустой каркас или отказ. Поэтому вкладка открывает адрес, как человек, а скрипт площадки ждёт готовых карточек.
pub async fn open<P: ScriptPage>(tab: &Tab, url: &str, script: &str, not_found: &str) -> Result<P, SourceError> {
    tab.evaluate::<bool>(MARK_PAGE_STALE).await?;
    tab.navigate(url).await?;
    let deadline = Instant::now() + PAGE_TIMEOUT;
    loop {
        sleep(POLL_INTERVAL).await;
        if let Ok(page) = tab.evaluate::<P>(script).await
            && !page.stale()
        {
            match page.state() {
                PageState::Loading => {}
                PageState::Captcha => return Err(SourceError::Captcha),
                PageState::NotFound => return Err(anyhow!("{not_found}").into()),
                PageState::Listings => return Ok(page),
            }
        }
        if Instant::now() >= deadline {
            return Err(SourceError::NotReady);
        }
    }
}

/// Первая страница — по адресу поиска или ссылки, следующие — тот же каталог с номером страницы, с паузой,
/// как у человека, который листает выдачу.
pub async fn read_pages<P: Listing>(
    tab: &Tab,
    url: &str,
    script: &str,
    not_found: &str,
    paging: &Paging,
) -> Result<Vec<Offer>, SourceError> {
    let mut page: P = open(tab, url, script, not_found).await?;
    if let Some(note) = page.note() {
        log::info!("{note}");
    }
    let mut catalog = Catalog::default();
    for number in 2..=paging.pages {
        let address = numbered(page.url(), paging.number_key, number)?;
        let cards = page.cards();
        if !catalog.add(cards, page.into_offers()?) {
            return Ok(catalog.offers);
        }
        sleep(paging.pause).await;
        page = open(tab, address.as_str(), script, not_found).await?;
    }
    let cards = page.cards();
    catalog.add(cards, page.into_offers()?);
    Ok(catalog.offers)
}

/// Адрес страницы выдачи с номером `number` в параметре `key`, остальное — как было.
fn numbered(url: &str, key: &str, number: u32) -> anyhow::Result<Url> {
    let mut url = Url::parse(url).context("площадка открыла непонятный адрес выдачи")?;
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(name, _)| name != key)
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect();
    url.query_pairs_mut()
        .clear()
        .extend_pairs(&kept)
        .append_pair(key, &number.to_string());
    Ok(url)
}

/// Товары со страниц выдачи. Дальше листать стоит, пока страница полная и приносит новое: неполная — последняя,
/// а без новых товаров площадка, видимо, показала за последней страницей уже виденное.
#[derive(Default)]
struct Catalog {
    offers: Vec<Offer>,
    page_size: usize,
}

impl Catalog {
    fn add(&mut self, cards: usize, offers: Vec<Offer>) -> bool {
        let before = self.offers.len();
        for offer in offers {
            if !self.offers.iter().any(|seen| seen.url == offer.url) {
                self.offers.push(offer);
            }
        }
        let full = cards > 0 && cards >= self.page_size;
        self.page_size = self.page_size.max(cards);
        full && self.offers.len() > before
    }
}

/// «6 399 ₽», «4 394 ₽» (с неразрывными пробелами) → рубли; ноль — не цена.
pub fn rubles(text: &str) -> Option<u64> {
    count(text)
}

/// «· 196 оценок», «1 043» → число; ноль — как будто его не написали.
pub fn count(text: &str) -> Option<u64> {
    let digits: String = text.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok().filter(|&number| number > 0)
}

/// «4.53», «4,9» → рейтинг.
pub fn rating(text: &str) -> Option<f32> {
    text.trim().replace(',', ".").parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offers(numbers: std::ops::Range<u32>) -> Vec<Offer> {
        numbers
            .map(|number| Offer {
                url: format!("https://www.dns-shop.ru/product/{number}/"),
                ..Offer::sample("Шуруповерт Makita", 6_399)
            })
            .collect()
    }

    #[test]
    fn next_page_is_the_same_catalog_with_its_number() {
        let catalog = "https://www.dns-shop.ru/catalog/17a9c6ec16404e77/surupoverty-dreli-surupoverty/\
                       ?q=makita&stock=now-today-tomorrow-later-out_of_stock&order=popular";
        assert_eq!(numbered(catalog, "p", 2).unwrap().as_str(), format!("{catalog}&p=2"));
        assert_eq!(
            numbered(&format!("{catalog}&p=2"), "p", 3).unwrap().as_str(),
            format!("{catalog}&p=3"),
            "номер из ссылки заменяется, а не дублируется"
        );
        assert_eq!(
            numbered("https://market.yandex.ru/search?text=sony", "page", 2)
                .unwrap()
                .as_str(),
            "https://market.yandex.ru/search?text=sony&page=2"
        );
    }

    #[test]
    fn pages_are_read_while_full_and_bringing_new_offers() {
        let mut catalog = Catalog::default();
        assert!(catalog.add(24, offers(0..24)));
        assert!(catalog.add(24, offers(24..48)));
        assert!(!catalog.add(7, offers(48..55)), "неполная страница — последняя");
        assert_eq!(catalog.offers.len(), 55);

        let mut repeated = Catalog::default();
        repeated.add(24, offers(0..24));
        assert!(
            !repeated.add(24, offers(0..24)),
            "за последней страницей — уже виденное"
        );
        assert_eq!(repeated.offers.len(), 24);
    }

    #[test]
    fn rubles_ignore_spaces_and_sign_but_not_zero() {
        assert_eq!(rubles("4\u{a0}394\u{a0}₽"), Some(4_394));
        assert_eq!(rubles("6 399 ₽"), Some(6_399));
        assert_eq!(rubles("0 ₽"), None);
        assert_eq!(rubles("—"), None);
    }
}
