use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::browser::{Browser, Tab};
use crate::marketplace::{Area, OfferSource, SourceError};
use crate::offer::Offer;
use crate::wait;

const HOME: &str = "https://www.ozon.ru/";
/// Скачивание порций выдачи; разбор карточек ему передаётся отдельным скриптом, который тесты проверяют без сети.
const SCAN_SCRIPT: &str = include_str!("ozon/scan.js");
const PARSE_SCRIPT: &str = include_str!("ozon/parse.js");
const READY_TIMEOUT: Duration = Duration::from_mins(1);
const SCAN_TIMEOUT: Duration = Duration::from_mins(2);

// У страницы проверки «я не робот» нет блоков data-widget, у настоящей страницы Ozon они есть.
const PAGE_IS_READY: &str = "location.href.startsWith('https://www.ozon.ru') \
                             && document.readyState === 'complete' \
                             && !!document.querySelector('[data-widget]')";

pub struct Ozon {
    browser: Arc<Browser>,
    portions: u32,
    tab_in_use: Mutex<()>,
}

#[derive(Deserialize)]
struct ScanResult {
    captcha: bool,
    offers: Vec<Offer>,
}

impl Ozon {
    /// `portions` — сколько порций выдачи просматривать; Ozon отдаёт их по 8 товаров.
    pub fn new(browser: Arc<Browser>, portions: u32) -> Self {
        Self {
            browser,
            portions,
            tab_in_use: Mutex::new(()),
        }
    }

    pub async fn open(&self) -> Result<(), SourceError> {
        self.ready_tab().await.map(drop)
    }

    async fn ready_tab(&self) -> Result<Tab, SourceError> {
        let tab = self.browser.tab(HOME).await?;
        if wait::until(READY_TIMEOUT, || page_is_ready(&tab)).await {
            Ok(tab)
        } else {
            Err(SourceError::NotReady)
        }
    }

    fn scan_expression(&self, query: &str) -> anyhow::Result<String> {
        let query = serde_json::to_string(query).context("не удалось подготовить запрос")?;
        Ok(format!("({SCAN_SCRIPT})({query}, {}, {PARSE_SCRIPT})", self.portions))
    }
}

#[async_trait]
impl OfferSource for Ozon {
    fn name(&self) -> &'static str {
        "Ozon"
    }

    async fn search(&self, query: &str, _area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.ready_tab().await?;
        let scan: ScanResult = timeout(SCAN_TIMEOUT, tab.evaluate(&self.scan_expression(query)?))
            .await
            .map_err(|_| anyhow!("Ozon не ответил за {} секунд", SCAN_TIMEOUT.as_secs()))??;

        if scan.captcha && scan.offers.is_empty() {
            return Err(SourceError::Captcha);
        }
        Ok(scan.offers)
    }
}

async fn page_is_ready(tab: &Tab) -> bool {
    tab.evaluate::<bool>(PAGE_IS_READY).await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::test_browser;

    const SAMPLE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ozon-portion.json");

    /// Те же виджеты, что ищет parse.js, — сетка и листалка — из первой порции выдачи.
    /// Рекламные метки просмотра (trackingInfo) разбору не нужны и в образец не попадают.
    const SAVE_PORTION: &str = r"(async query => {
      const withoutTracking = state => JSON.stringify(JSON.parse(state, (key, value) => key === 'trackingInfo' ? undefined : value));
      let path = '/search/?text=' + encodeURIComponent(query);
      for (let redirects = 0; redirects <= 3; redirects++) {
        const response = await fetch('/api/entrypoint-api.bx/page/json/v2?url=' + encodeURIComponent(path), { credentials: 'include' });
        const page = await response.json();
        if (!page.redirectTo) {
          const kept = Object.entries(page.widgetStates || {})
            .filter(([name]) => /^(tileGrid|searchResults)|aginator/.test(name))
            .map(([name, state]) => [name, withoutTracking(state)]);
          return { widgetStates: Object.fromEntries(kept) };
        }
        const target = new URL(page.redirectTo, location.origin);
        path = target.pathname + target.search;
      }
      return null;
    })('наушники sony')";

    /// Когда Ozon поменяет формат и тест разбора начнёт падать, этот тест сохраняет свежий образец.
    /// Только по явной просьбе: вместе с остальными живыми тестами он образец не трогает.
    #[tokio::test]
    #[ignore = "перезаписывает образец выдачи с живого Ozon, если задано SAVE_OZON_SAMPLE=1"]
    async fn saves_fresh_portion_sample() {
        if std::env::var_os("SAVE_OZON_SAMPLE").is_none() {
            println!("образец не тронут: для перезаписи задайте SAVE_OZON_SAMPLE=1");
            return;
        }
        let ozon = Ozon::new(test_browser(), 1);
        let tab = ozon.ready_tab().await.unwrap();

        let sample: serde_json::Value = tab.evaluate(SAVE_PORTION).await.unwrap();

        assert!(
            sample["widgetStates"]
                .as_object()
                .is_some_and(|widgets| !widgets.is_empty())
        );
        std::fs::create_dir_all(std::path::Path::new(SAMPLE_PATH).parent().unwrap()).unwrap();
        std::fs::write(SAMPLE_PATH, serde_json::to_string_pretty(&sample).unwrap()).unwrap();
    }

    #[derive(Deserialize)]
    struct ParsedPortion {
        offers: Vec<Offer>,
        next: Option<String>,
    }

    /// parse.js — тот же код, что работает во вкладке, — выполняется в движке JavaScript без браузера и сети.
    fn parse_portion(page: &str) -> ParsedPortion {
        let script = format!(
            "const parse = {PARSE_SCRIPT};
             const widgets = Object.entries(({page}).widgetStates);
             JSON.stringify({{
               offers: parse.gridItems(widgets).map(card => parse.toOffer(card, 'https://www.ozon.ru')).filter(Boolean),
               next: parse.nextPortionPath(widgets) ?? null,
             }})"
        );
        let mut engine = boa_engine::Context::default();
        let result = engine
            .eval(boa_engine::Source::from_bytes(&script))
            .unwrap_or_else(|error| panic!("parse.js упал: {error}"));
        let json = result.to_string(&mut engine).unwrap().to_std_string_escaped();
        serde_json::from_str(&json).unwrap()
    }

    /// Если упал после того, как Ozon поменял выдачу: поправьте parse.js, сохраните свежий образец
    /// (`saves_fresh_portion_sample`) и обновите здесь первую карточку.
    #[test]
    fn saved_portion_is_parsed_without_network() {
        let portion = parse_portion(&std::fs::read_to_string(SAMPLE_PATH).unwrap());

        assert_eq!(portion.offers.len(), 8, "в порции Ozon восемь карточек");
        assert_eq!(
            portion.offers[0],
            Offer {
                old_price: Some(7_990),
                rating: Some(4.9),
                reviews: Some(8_946),
                url: "https://www.ozon.ru/product/sony-wh-ch520-naushniki-nakladnye-besprovodnye-chernye-895366703/"
                    .into(),
                ..Offer::sample("Sony WH-CH520 Наушники накладные беспроводные, черные", 3_870)
            }
        );
        assert!(portion.offers.iter().all(|offer| {
            !offer.name.is_empty()
                && offer.rating.is_some()
                && offer.reviews.is_some()
                && offer.url.starts_with("https://www.ozon.ru/product/")
                && !offer.url.contains('?')
        }));
        assert!(portion.next.is_some_and(|path| path.contains("page=2")));
    }

    #[test]
    fn cards_without_link_or_price_are_skipped_and_name_falls_back_to_longest_text() {
        let card = |link: Option<&str>, texts: serde_json::Value| serde_json::json!({ "action": link.map(|link| serde_json::json!({ "link": link })), "mainState": texts });
        let grid = serde_json::json!({ "items": [
            card(
                Some("https://www.ozon.ru/product/chaynik-1/?at=abc"),
                serde_json::json!([
                    { "textAtom": { "text": "Чайник электрический стеклянный" } },
                    { "text": "1 990 ₽", "textStyle": "PRICE" },
                    { "text": "2 990 ₽", "textStyle": "ORIGINAL_PRICE" },
                ]),
            ),
            card(None, serde_json::json!([{ "text": "990 ₽", "textStyle": "PRICE" }])),
            card(Some("/product/bez-ceny-2/"), serde_json::json!([{ "textAtom": { "text": "Товар без цены вообще" } }])),
        ]});
        let sort = serde_json::json!({ "items": [card(Some("/sort/"), serde_json::json!([{ "text": "1 ₽" }]))] });
        let page = serde_json::json!({ "widgetStates": {
            "tileGridDesktop-1-default-1": grid.to_string(),
            "searchResultsSort-2-default-1": sort.to_string(),
        }});

        let portion = parse_portion(&page.to_string());

        assert_eq!(
            portion.offers,
            [Offer {
                old_price: Some(2_990),
                url: "https://www.ozon.ru/product/chaynik-1/".into(),
                ..Offer::sample("Чайник электрический стеклянный", 1_990)
            }]
        );
        assert!(portion.next.is_none());
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Ozon: cargo test -- --ignored --test-threads=1"]
    async fn finds_offers_on_live_site() {
        let ozon = Ozon::new(test_browser(), 2);

        let offers = ozon.search("наушники sony", None).await.unwrap();

        assert!(offers.len() >= 10, "собрано всего {}", offers.len());
        assert!(
            offers
                .iter()
                .all(|o| !o.name.is_empty() && o.price > 0 && o.url.starts_with("https://www.ozon.ru/product/"))
        );
        assert!(offers.iter().any(|o| o.rating.is_some() && o.reviews.is_some()));
        println!(
            "собрано {}, первое: {} ₽ {}",
            offers.len(),
            offers[0].price,
            offers[0].name
        );
    }
}
