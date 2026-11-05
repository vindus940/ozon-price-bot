use std::collections::HashSet;

use crate::hunt::assess::is_copy;
use crate::hunt::category::{Category, looks_used};
use crate::hunt::deal::suspiciously_cheap;
use crate::hunt::failure_reason;
use crate::hunt::market::Market;
use crate::marketplace::{SourceError, SourceKind};
use crate::offer::Offer;
use crate::search::{Findings, OfferSearch, SearchRequest};

/// Что ответил поиск цены: тот же товар со всех площадок, от дешёвого к дорогому.
pub struct BestPrices {
    pub scanned: usize,
    /// Подошли по словам запроса, но это другой товар: аксессуар, другая версия, брак.
    pub other_product: usize,
    pub offers: Vec<PricedOffer>,
    pub market: Option<Market>,
    /// Самая низкая честная цена каждой площадки, от дешёвой к дорогой.
    pub lowest: Vec<(SourceKind, u64)>,
    /// Площадки, которые не ответили, и почему.
    pub failed: Vec<(SourceKind, String)>,
}

pub struct PricedOffer {
    pub source: SourceKind,
    pub offer: Offer,
    pub used: bool,
    /// Дешевле половины рынка: скорее подделка или ошибка, чем выгода.
    pub suspicious: bool,
}

impl BestPrices {
    /// Лучшая цена магазина. Состояние объявления Авито в выдаче не пишется, поэтому его цена — отдельно, `best_ad`:
    /// по живому поиску самым дешёвым «новым» оказывалось объявление, про которое это неизвестно.
    pub fn best(&self) -> Option<&PricedOffer> {
        self.offers
            .iter()
            .find(|found| !found.suspicious && !found.used && !found.source.has_used_goods())
    }

    pub fn best_ad(&self) -> Option<&PricedOffer> {
        self.offers
            .iter()
            .find(|found| !found.suspicious && found.source.has_used_goods())
    }
}

/// Поиск цены в два прохода. Популярная выдача показывает рынок, но самое дешёвое бывает в ней далеко: поэтому второй
/// проход — от дешёвых, в коридоре от половины рынка (ниже — чехлы и подделки) до самого рынка. Яндекс Маркет
/// спрашивается один раз, сразу вторым проходом: коридор ему дают остальные.
pub async fn hunt_best_prices(
    search: &OfferSearch,
    request: &SearchRequest,
    category: Category,
    new_only: bool,
) -> Result<BestPrices, (SourceKind, SourceError)> {
    let (regular, once) = search.split_asked_once();
    if regular.is_empty() {
        return Ok(best_prices(category, request, once.find(request).await?, new_only));
    }
    let mut findings = regular.find(request).await?;
    // Рынок — только по популярной выдаче: второй проход нарочно собирает дешёвое и занизил бы его.
    let market = market_of(category, request, &findings, new_only);
    match market {
        Some(market) => {
            let mut deeper = search.find_cheapest(request, market.median / 2, market.median).await;
            // Площадка уже ответила первым проходом — её предложения в списке, и пугать оговоркой незачем.
            deeper.failed.retain(|(source, _)| source.is_asked_once());
            findings.merge(deeper, request);
        }
        None if !once.is_empty() => match once.find(request).await {
            Ok(popular) => findings.merge(popular, request),
            Err(failure) => findings.failed.push(failure),
        },
        None => {}
    }
    Ok(priced(category, request, findings, new_only, market))
}

fn market_of(category: Category, request: &SearchRequest, findings: &Findings, new_only: bool) -> Option<Market> {
    let prices: Vec<u64> = findings
        .suitable
        .iter()
        .filter(|found| category.accepts(&request.query, &found.offer))
        .filter(|found| !(new_only && looks_used(&found.offer)))
        .map(|found| found.offer.price)
        .collect();
    Market::of(&prices)
}

fn best_prices(category: Category, request: &SearchRequest, findings: Findings, new_only: bool) -> BestPrices {
    priced(category, request, findings, new_only, None)
}

/// Отбор тот же, что у слежек: иначе самым дешёвым «товаром» всегда окажется чехол к нему.
/// `market` — рынок, если он уже известен; иначе считается по тому, что найдено.
fn priced(
    category: Category,
    request: &SearchRequest,
    findings: Findings,
    new_only: bool,
    market: Option<Market>,
) -> BestPrices {
    let scanned = findings.scanned;
    let failed = findings
        .failed
        .iter()
        .map(|(source, error)| (*source, failure_reason(*source, error)))
        .collect();

    let mut seen = HashSet::new();
    let mut other_product = 0;
    let mut same = Vec::new();
    for found in findings.suitable {
        if !category.accepts(&request.query, &found.offer) {
            other_product += 1;
            continue;
        }
        let used = looks_used(&found.offer);
        let unwanted = new_only && used;
        if !unwanted && !is_copy(&mut seen, &found.offer) {
            same.push((found.source, found.offer, used));
        }
    }

    let prices: Vec<u64> = same.iter().map(|(_, offer, _)| offer.price).collect();
    let market = market.or_else(|| Market::of(&prices));
    let offers: Vec<PricedOffer> = same
        .into_iter()
        .map(|(source, offer, used)| PricedOffer {
            suspicious: market.is_some_and(|market| suspiciously_cheap(market.median, offer.price)),
            source,
            offer,
            used,
        })
        .collect();
    let lowest = lowest_by_source(&offers);
    BestPrices {
        scanned,
        other_product,
        offers,
        market,
        lowest,
        failed,
    }
}

fn lowest_by_source(offers: &[PricedOffer]) -> Vec<(SourceKind, u64)> {
    let mut lowest: Vec<(SourceKind, u64)> = Vec::new();
    for found in offers.iter().filter(|found| !found.suspicious && !found.used) {
        match lowest.iter_mut().find(|(source, _)| *source == found.source) {
            Some((_, price)) => *price = found.offer.price.min(*price),
            None => lowest.push((found.source, found.offer.price)),
        }
    }
    lowest.sort_by_key(|&(_, price)| price);
    lowest
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::marketplace::testing::FakeSource;
    use crate::search::Finding;

    fn found(source: SourceKind, name: &str, price: u64) -> Finding {
        Finding {
            source,
            offer: Offer {
                url: format!("https://example.com/{source:?}/{price}"),
                ..Offer::sample(name, price)
            },
            unit_price: None,
        }
    }

    fn findings(suitable: Vec<Finding>) -> Findings {
        Findings {
            scanned: suitable.len(),
            suitable,
            packed_in: None,
            failed: Vec::new(),
        }
    }

    fn request() -> SearchRequest {
        SearchRequest::parse("наушники sony wh-ch520")
    }

    fn market_of_five() -> Vec<Finding> {
        vec![
            found(SourceKind::Wildberries, "Наушники Sony WH-CH520", 3_900),
            found(SourceKind::Ozon, "Наушники Sony WH-CH520", 4_000),
            found(SourceKind::YandexMarket, "Наушники Sony WH-CH520", 4_100),
            found(SourceKind::Ozon, "Наушники Sony WH-CH520 белые", 4_200),
            found(SourceKind::Dns, "Наушники Sony WH-CH520", 4_400),
        ]
    }

    fn sony(prices: &[u64]) -> Vec<Offer> {
        prices
            .iter()
            .map(|price| Offer {
                url: format!("https://example.com/{price}"),
                ..Offer::sample("Наушники Sony WH-CH520", *price)
            })
            .collect()
    }

    async fn hunted(search: &OfferSearch) -> BestPrices {
        hunt_best_prices(search, &request(), Category::Electronics, false)
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"))
    }

    #[tokio::test]
    async fn cheapest_hidden_behind_popular_offers_is_found_in_the_corridor() {
        let source = FakeSource::returning(sony(&[3_900, 4_000, 4_100, 4_200, 4_400])).with_cheapest(sony(&[2_500]));
        let asked = source.queries();
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(source))]);

        let prices = hunted(&search).await;

        assert_eq!(prices.best().unwrap().offer.price, 2_500);
        assert_eq!(
            prices.market.unwrap().median,
            4_100,
            "дешёвое со второго прохода рынок не занижает"
        );
        assert_eq!(
            *asked.lock().unwrap(),
            ["наушники sony wh-ch520", "наушники sony wh-ch520 2050..4100"],
            "коридор — от половины рынка до рынка"
        );
    }

    #[tokio::test]
    async fn same_offer_from_both_passes_counts_once() {
        let source = FakeSource::returning(sony(&[3_900, 4_000, 4_100, 4_200, 4_400])).with_cheapest(sony(&[3_900]));
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(source))]);

        assert_eq!(hunted(&search).await.offers.len(), 5);
    }

    #[tokio::test]
    async fn market_is_asked_once_and_straight_for_the_cheapest() {
        let ozon = FakeSource::returning(sony(&[3_900, 4_000, 4_100, 4_200, 4_400]));
        let market = FakeSource::returning(sony(&[9_999])).with_cheapest(sony(&[2_600]));
        let asked = market.queries();
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(ozon)),
            (SourceKind::YandexMarket, Arc::new(market)),
        ]);

        let prices = hunted(&search).await;

        assert_eq!(prices.best().unwrap().source, SourceKind::YandexMarket);
        assert_eq!(*asked.lock().unwrap(), ["наушники sony wh-ch520 2050..4100"]);
    }

    #[tokio::test]
    async fn without_a_market_the_market_source_is_asked_the_usual_way() {
        let ozon = FakeSource::returning(sony(&[3_900]));
        let market = FakeSource::returning(sony(&[4_100]));
        let asked = market.queries();
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(ozon)),
            (SourceKind::YandexMarket, Arc::new(market)),
        ]);

        let prices = hunted(&search).await;

        assert_eq!(prices.offers.len(), 2);
        assert_eq!(*asked.lock().unwrap(), ["наушники sony wh-ch520"]);
    }

    #[tokio::test]
    async fn market_alone_is_not_asked_twice() {
        let market = FakeSource::returning(sony(&[3_900, 4_000, 4_100, 4_200, 4_400]));
        let asked = market.queries();
        let search = OfferSearch::new(vec![(SourceKind::YandexMarket, Arc::new(market))]);

        hunted(&search).await;

        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn silence_on_the_second_pass_warns_only_about_the_source_missing_from_the_list() {
        let ozon = FakeSource::returning(sony(&[3_900, 4_000, 4_100, 4_200, 4_400]));
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(ozon)),
            (SourceKind::YandexMarket, Arc::new(FakeSource::asking_captcha())),
        ]);

        let prices = hunted(&search).await;

        assert_eq!(prices.failed.len(), 1);
        assert_eq!(prices.failed[0].0, SourceKind::YandexMarket);
    }

    #[test]
    fn best_price_comes_from_all_sources_and_each_shows_its_lowest() {
        let prices = best_prices(Category::Electronics, &request(), findings(market_of_five()), false);

        let best = prices.best().unwrap();
        assert_eq!((best.source, best.offer.price), (SourceKind::Wildberries, 3_900));
        assert_eq!(
            prices.lowest,
            [
                (SourceKind::Wildberries, 3_900),
                (SourceKind::Ozon, 4_000),
                (SourceKind::YandexMarket, 4_100),
                (SourceKind::Dns, 4_400)
            ]
        );
        assert_eq!(prices.market.unwrap().median, 4_100);
    }

    #[test]
    fn accessory_is_not_the_cheapest_product() {
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Ozon, "Чехол для наушников Sony WH-CH520", 300));

        let prices = best_prices(Category::Electronics, &request(), findings(offers), false);

        assert_eq!(prices.best().unwrap().offer.price, 3_900);
        assert_eq!(prices.other_product, 1);
        assert_eq!(prices.scanned, 6);
    }

    #[test]
    fn suspiciously_cheap_is_shown_but_is_not_the_best_price() {
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, "Наушники Sony WH-CH520", 900));

        let prices = best_prices(Category::Electronics, &request(), findings(offers), false);

        assert!(prices.offers[0].suspicious);
        assert_eq!(prices.best().unwrap().offer.price, 3_900);
        assert!(!prices.lowest.iter().any(|(source, _)| *source == SourceKind::Avito));
    }

    #[test]
    fn ad_without_a_word_about_condition_is_not_the_best_shop_price() {
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, "Наушники Sony WH-CH520", 2_500));

        let prices = best_prices(Category::Electronics, &request(), findings(offers), false);

        assert_eq!(prices.best().unwrap().offer.price, 3_900);
        assert_eq!(prices.best_ad().unwrap().offer.price, 2_500);
    }

    #[test]
    fn used_goes_apart_from_the_best_shop_price_or_away_when_asked() {
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, "Наушники Sony WH-CH520 б/у", 2_500));

        let prices = best_prices(Category::Electronics, &request(), findings(offers), false);
        assert_eq!(prices.best().unwrap().offer.price, 3_900);
        assert_eq!(prices.best_ad().unwrap().offer.price, 2_500);

        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, "Наушники Sony WH-CH520 б/у", 2_500));
        let new_only = best_prices(Category::Electronics, &request(), findings(offers), true);
        assert!(new_only.best_ad().is_none());
        assert_eq!(new_only.offers.len(), 5);
    }

    #[test]
    fn pack_of_identical_ads_counts_once() {
        let copy = |number: u32| Finding {
            offer: Offer {
                url: format!("https://www.avito.ru/{number}"),
                place: Some("Валовая ул.".into()),
                ..Offer::sample("Наушники Sony WH-CH520", 3_000)
            },
            ..found(SourceKind::Avito, "", 1)
        };

        let prices = best_prices(
            Category::Electronics,
            &request(),
            findings(vec![copy(1), copy(2)]),
            false,
        );

        assert_eq!(prices.offers.len(), 1);
    }

    #[test]
    fn silent_source_is_named_with_the_reason() {
        let answer = Findings {
            failed: vec![(SourceKind::YandexMarket, SourceError::Captcha)],
            ..findings(market_of_five())
        };

        let prices = best_prices(Category::Electronics, &request(), answer, false);

        assert_eq!(prices.failed.len(), 1);
        assert!(prices.failed[0].1.contains("не робот"), "{}", prices.failed[0].1);
    }

    #[test]
    fn few_offers_give_no_market_and_nothing_is_suspicious() {
        let offers = vec![found(SourceKind::Ozon, "Наушники Sony WH-CH520", 900)];

        let prices = best_prices(Category::Electronics, &request(), findings(offers), false);

        assert!(prices.market.is_none());
        assert_eq!(prices.best().unwrap().offer.price, 900);
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на все живые площадки: cargo test -- --ignored --test-threads=1"]
    async fn best_price_over_all_live_sites() {
        use crate::marketplace::avito::Avito;
        use crate::marketplace::dns::Dns;
        use crate::marketplace::ozon::Ozon;
        use crate::marketplace::wildberries::Wildberries;
        use crate::marketplace::yandex_market::YandexMarket;
        use crate::storage::Storage;

        let browser = crate::browser::test_browser();
        let storage = Storage::new(std::env::temp_dir().join("ozon-price-bot-best-price-test"));
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(Ozon::new(Arc::clone(&browser), 3))),
            (
                SourceKind::Wildberries,
                Arc::new(Wildberries::new(Arc::clone(&browser))),
            ),
            (
                SourceKind::YandexMarket,
                Arc::new(YandexMarket::new(Arc::clone(&browser))),
            ),
            (
                SourceKind::Avito,
                Arc::new(Avito::new(Arc::clone(&browser), "moskva", storage)),
            ),
            (SourceKind::Dns, Arc::new(Dns::new(browser))),
        ]);
        let request = SearchRequest::parse("наушники sony wh-ch520");

        let prices = hunt_best_prices(&search, &request, Category::Electronics, false)
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));

        println!(
            "просмотрено {}, тот же товар {}, другой {}",
            prices.scanned,
            prices.offers.len(),
            prices.other_product
        );
        println!("рынок {:?}, по площадкам {:?}", prices.market, prices.lowest);
        println!("не ответили: {:?}", prices.failed);
        for found in [prices.best(), prices.best_ad()].into_iter().flatten() {
            println!("{:?} {} ₽ {}", found.source, found.offer.price, found.offer.name);
        }
        assert!(prices.best().is_some());
    }
}
