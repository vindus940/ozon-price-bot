use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow};
use async_trait::async_trait;
use reqwest::Url;
use serde::Deserialize;
use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::browser::{Browser, Tab};
use crate::marketplace::{Area, OfferSource, Product, SourceError};
use crate::offer::Offer;
use crate::wait;

const HOME: &str = "https://www.ozon.ru/";
/// Скачивание порций выдачи; разбор карточек ему передаётся отдельным скриптом, который тесты проверяют без сети.
const SCAN_SCRIPT: &str = include_str!("ozon/scan.js");
const PARSE_SCRIPT: &str = include_str!("ozon/parse.js");
const PRODUCT_SCRIPT: &str = include_str!("ozon/product.js");
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

#[derive(Deserialize)]
struct ProductResult {
    captcha: bool,
    product: Option<Product>,
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

    /// `start` — путь выдачи из ссылки слежки; без него поиск по словам.
    fn scan_expression(&self, query: &str, start: Option<&str>) -> anyhow::Result<String> {
        let query = serde_json::to_string(query).context("не удалось подготовить запрос")?;
        let start = serde_json::to_string(&start).context("не удалось подготовить ссылку")?;
        Ok(format!(
            "({SCAN_SCRIPT})({query}, {}, {PARSE_SCRIPT}, {start})",
            self.portions
        ))
    }

    async fn scan(&self, expression: &str) -> Result<Vec<Offer>, SourceError> {
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.ready_tab().await?;
        let scan: ScanResult = timeout(SCAN_TIMEOUT, tab.evaluate(expression))
            .await
            .map_err(|_| anyhow!("Ozon не ответил за {} секунд", SCAN_TIMEOUT.as_secs()))??;

        if scan.captcha && scan.offers.is_empty() {
            return Err(SourceError::Captcha);
        }
        Ok(scan.offers)
    }
}

#[async_trait]
impl OfferSource for Ozon {
    fn name(&self) -> &'static str {
        "Ozon"
    }

    async fn search(&self, query: &str, _area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
        self.scan(&self.scan_expression(query, None)?).await
    }

    /// Ozon отдаёт выдачу по любому её адресу тем же запросом из вкладки, поэтому ссылка — это просто другой путь.
    async fn search_link(&self, link: &Url, _area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
        let path = match link.query() {
            Some(query) => format!("{}?{query}", link.path()),
            None => link.path().to_string(),
        };
        self.scan(&self.scan_expression("", Some(&path))?).await
    }

    async fn product(&self, link: &Url) -> Result<Product, SourceError> {
        let path = serde_json::to_string(link.path()).context("не удалось подготовить ссылку")?;
        let expression = format!("({PRODUCT_SCRIPT})({path}, {PARSE_SCRIPT})");
        let _exclusive = self.tab_in_use.lock().await;
        let tab = self.ready_tab().await?;
        let result: ProductResult = timeout(SCAN_TIMEOUT, tab.evaluate(&expression))
            .await
            .map_err(|_| anyhow!("Ozon не ответил за {} секунд", SCAN_TIMEOUT.as_secs()))??;
        match result {
            ProductResult { captcha: true, .. } => Err(SourceError::Captcha),
            ProductResult {
                product: Some(product), ..
            } => Ok(product),
            ProductResult { product: None, .. } => Err(anyhow!(
                "Ozon не показал товар по ссылке — откройте её в браузере: может быть, товар удалён"
            )
            .into()),
        }
    }
}

async fn page_is_ready(tab: &Tab) -> bool {
    tab.evaluate::<bool>(PAGE_IS_READY).await.unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::test_browser;
    use crate::marketplace::SourceKind;

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

    const PRODUCT_SAMPLE_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/ozon-product.json");

    fn parse_product(page: &serde_json::Value) -> Option<Product> {
        let script = format!(
            "const parse = {PARSE_SCRIPT};
             JSON.stringify(parse.productOf(Object.entries(({page}).widgetStates)))"
        );
        let mut engine = boa_engine::Context::default();
        let result = engine
            .eval(boa_engine::Source::from_bytes(&script))
            .unwrap_or_else(|error| panic!("parse.js упал: {error}"));
        serde_json::from_str(&result.to_string(&mut engine).unwrap().to_std_string_escaped()).unwrap()
    }

    fn product_page(price: &serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "widgetStates": {
            "webProductHeading-1-default-1": serde_json::json!({ "title": "Чайник  Xiaomi" }).to_string(),
            "webPrice-2-default-1": price.to_string(),
        }})
    }

    /// Образец — страница наушников 29.10.2026: с Ozon Картой 3 870 ₽ (как на плитке выдачи), без неё — 3 990 ₽.
    #[test]
    fn saved_product_page_gives_both_prices() {
        let page = serde_json::from_str(&std::fs::read_to_string(PRODUCT_SAMPLE_PATH).unwrap()).unwrap();
        assert_eq!(
            parse_product(&page),
            Some(Product {
                name: "Sony WH-CH520 Наушники накладные беспроводные, черные".into(),
                price: Some(3_870),
                price_without_card: Some(3_990),
            })
        );
    }

    #[test]
    fn product_out_of_stock_has_no_price_and_single_price_is_for_everyone() {
        let sold_out =
            product_page(&serde_json::json!({ "isAvailable": false, "cardPrice": "1 990 ₽", "price": "2 090 ₽" }));
        assert_eq!(parse_product(&sold_out).map(|product| product.price), Some(None));

        let single = product_page(&serde_json::json!({ "isAvailable": true, "price": "2 090 ₽" }));
        assert_eq!(
            parse_product(&single),
            Some(Product {
                name: "Чайник Xiaomi".into(),
                price: Some(2_090),
                price_without_card: None,
            })
        );
        assert_eq!(
            parse_product(&serde_json::json!({ "widgetStates": {} })),
            None,
            "не страница товара"
        );
    }

    fn is_robot_check(html: &str) -> bool {
        let script = format!(
            "const parse = {PARSE_SCRIPT}; parse.isRobotCheck({})",
            serde_json::to_string(html).unwrap()
        );
        let mut engine = boa_engine::Context::default();
        engine
            .eval(boa_engine::Source::from_bytes(&script))
            .unwrap_or_else(|error| panic!("parse.js упал: {error}"))
            .to_boolean()
    }

    #[test]
    fn service_antibot_params_of_every_page_are_not_a_robot_check() {
        let empty_search = r#"<div id="state-searchBarDesktop-3414131"></div>
            <script>{"o3Params":{"x-o3-antibot-score":"0","x-o3-antibot-processed":"1"}}</script>"#;
        assert!(
            !is_robot_check(empty_search),
            "так выглядит любая страница Ozon, например поиск «1» без товаров"
        );
        assert!(is_robot_check("<title>Доступ ограничен</title>"));
        assert!(is_robot_check(
            r#"<div class="antibot-challenge">Подтвердите, что вы не робот</div>"#
        ));
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
                delivery: Some("Завтра".into()),
                sold_by_ozon: true,
                brand_verified: true,
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
        assert_eq!(
            portion.offers[1].delivery.as_deref(),
            Some("За час"),
            "у экспресса своя кнопка корзины"
        );
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
        assert!(
            offers.iter().any(|o| o.delivery.is_some()),
            "пропал срок доставки с плиток"
        );
        println!(
            "собрано {}, первое: {} ₽ {}",
            offers.len(),
            offers[0].price,
            offers[0].name
        );
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Ozon: cargo test -- --ignored --test-threads=1"]
    async fn reads_product_page_on_live_site() {
        let ozon = Ozon::new(test_browser(), 1);
        let link = SourceKind::Ozon
            .parse_product_link(
                "https://www.ozon.ru/product/sony-wh-ch520-naushniki-nakladnye-besprovodnye-chernye-895366703/",
            )
            .unwrap();

        let product = ozon.product(&link).await.unwrap();

        assert!(product.name.contains("WH-CH520"), "{product:?}");
        assert!(product.price.is_some(), "пропала цена со страницы товара: {product:?}");
        println!("{product:?}");
    }

    /// Ссылка с фильтрами «бренд Xiaomi» (номер бренда на Ozon) и «доставка завтра»: без фильтров Xiaomi среди
    /// наушников меньшинство, так что тест падает, если фильтры ссылки потеряются — на первой порции или при листании.
    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Ozon: cargo test -- --ignored --test-threads=1"]
    async fn finds_offers_by_filtered_link_on_live_site() {
        let ozon = Ozon::new(test_browser(), 2);
        let link = SourceKind::Ozon
            .parse_link("https://www.ozon.ru/category/audiotehnika-15543/?text=наушники&brand=32686750&delivery=2")
            .unwrap();

        let offers = ozon.search_link(&link, None).await.unwrap();

        assert!(
            offers.len() > 8,
            "собрано всего {} — вторая порция не пришла",
            offers.len()
        );
        let foreign: Vec<&str> = offers
            .iter()
            .filter(|o| !o.name.to_lowercase().contains("xiaomi") && !o.name.to_lowercase().contains("redmi"))
            .map(|o| o.name.as_str())
            .collect();
        assert!(foreign.is_empty(), "фильтр бренда потерялся: {foreign:?}");
        assert!(
            offers.iter().all(Offer::arrives_by_tomorrow),
            "фильтр доставки потерялся"
        );
    }
}
