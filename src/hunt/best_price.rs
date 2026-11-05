use std::collections::{BTreeSet, HashMap, HashSet};

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;

use crate::hunt::assess::is_copy;
use crate::hunt::category::{Category, looks_used};
use crate::hunt::deal::suspiciously_cheap;
use crate::hunt::failure_reason;
use crate::hunt::market::Market;
use crate::hunt::model::{Foldable, fold_models, model_key};
use crate::marketplace::{SourceError, SourceKind};
use crate::offer::Offer;
use crate::search::{Finding, Findings, OfferFilter, OfferSearch, SearchRequest};

/// Из двух цен не понять, какая из них обычная: дешёвая может быть честной, а дорогая — перекупом.
const OFFERS_TO_KNOW_MODEL_PRICE: usize = 3;

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
    /// Дешевле половины обычной цены этой модели, а если она неизвестна — рынка: скорее подделка или ошибка, чем выгода.
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

    /// Предложения одной модели вместе, на месте первого из них (`fold_models`).
    pub fn groups(&self) -> Vec<Vec<&PricedOffer>> {
        fold_models(&self.offers, |found| Foldable {
            offer: &found.offer,
            apart: found.used || found.source.has_used_goods(),
            suspicious: found.suspicious,
        })
    }
}

/// То же для ответа бота: б/у он отдельно не считает, поэтому узнаётся по тексту предложения.
pub fn same_model_groups(findings: &Findings) -> Vec<Vec<&Finding>> {
    fold_models(&findings.suitable, |found| Foldable {
        offer: &found.offer,
        apart: found.source.has_used_goods() || looks_used(&found.offer),
        suspicious: found.suspicious,
    })
}

/// Что уже найдено, пока остальные площадки думают: быстрая площадка не должна ждать медленную.
pub struct PriceProgress {
    pub prices: BestPrices,
    pub waiting: Vec<SourceKind>,
}

/// Как идёт поиск бота: сообщение «Ищу…» обновляется, иначе за второй проход кажется, что бот завис.
#[derive(Debug, PartialEq)]
pub struct OffersProgress {
    /// Сколько уже подошло под запрос со всеми его условиями.
    pub suitable: usize,
    pub waiting: Vec<SourceKind>,
    /// Первые ответы получены, площадки читают выдачу от дешёвых.
    pub deeper: bool,
}

/// Поиск цены для окна: `report` получает промежуточный итог после ответа каждой площадки.
pub async fn hunt_best_prices(
    search: &OfferSearch,
    request: &SearchRequest,
    category: Category,
    new_only: bool,
    report: &(dyn Fn(PriceProgress) + Send + Sync),
) -> Result<BestPrices, (SourceKind, SourceError)> {
    let tell = |findings: &Findings, market, waiting: &[SourceKind]| {
        report(PriceProgress {
            prices: priced(category, request, findings, new_only, market),
            waiting: waiting.to_vec(),
        });
    };
    let (findings, market) = two_passes(search, request, category, new_only, tell).await?;
    Ok(priced(category, request, &findings, new_only, market))
}

/// Поиск для бота: тот же второй проход, но ответ — весь список, а не лучшая цена. Категории бот не спрашивает,
/// поэтому рынок — по всему, что подошло по словам. «По скидке» и «за кг» спрашивают не про самую низкую цену
/// предложения: заход на площадки ради них был бы лишним.
pub async fn hunt_offers(
    search: &OfferSearch,
    request: &SearchRequest,
    report: impl Fn(OffersProgress) + Send + Sync,
) -> Result<Findings, (SourceKind, SourceError)> {
    if !request.wants_lowest_price() {
        let findings = search.find_for_market(request).await?;
        return Ok(answer_of_bot(findings, request, None));
    }
    let wanted = OfferFilter::new(request);
    let tell = |findings: &Findings, market: Option<Market>, waiting: &[SourceKind]| {
        report(OffersProgress {
            suitable: findings
                .suitable
                .iter()
                .filter(|found| wanted.accepts(&found.offer))
                .count(),
            waiting: waiting.to_vec(),
            deeper: market.is_some(),
        });
    };
    let (findings, market) = two_passes(search, request, Category::Any, false, tell).await?;
    Ok(answer_of_bot(findings, request, market))
}

/// Вещи «для» искомого и брак уходят из ответа: это другой товар, и он не должен стоять первым как самая низкая цена.
/// Сначала судим цены: для этого нужны и те предложения, что в ответ не попадут.
fn answer_of_bot(mut findings: Findings, request: &SearchRequest, market: Option<Market>) -> Findings {
    mark_suspicious(&mut findings, request, market);
    findings
        .suitable
        .retain(|found| Category::Any.accepts(&request.query, &found.offer));
    findings.refine(request);
    findings
}

/// До отбора по условиям запроса: при «до 5000» настоящие цены дорогой модели в ответ не попадут, а судить копии нужны.
fn mark_suspicious(findings: &mut Findings, request: &SearchRequest, market: Option<Market>) {
    // Рынок фасованного — рынок мешков и пачек вперемешку: маленькая пачка рядом с ним не подозрительна.
    let packed = findings.packed_in.is_some() || request.per_unit.is_some();
    let market = market
        .or_else(|| market_of(Category::Any, request, findings, false))
        .filter(|_| !packed);
    let usual = usual_model_prices(Category::Any, request, findings);
    let many_models = model_key(&request.query).is_none();
    for found in &mut findings.suitable {
        found.suspicious = price_to_compare(&found.offer, &usual, market, many_models)
            .is_some_and(|usual| suspiciously_cheap(usual, found.offer.price));
    }
}

/// Популярная выдача показывает рынок, но самое дешёвое бывает в ней далеко: поэтому второй проход — от дешёвых,
/// в коридоре от половины рынка (ниже — чехлы и подделки) до самого рынка. Яндекс Маркет спрашивается один раз,
/// сразу вторым проходом: коридор ему дают остальные.
async fn two_passes(
    search: &OfferSearch,
    request: &SearchRequest,
    category: Category,
    new_only: bool,
    tell: impl Fn(&Findings, Option<Market>, &[SourceKind]) + Send + Sync,
) -> Result<(Findings, Option<Market>), (SourceKind, SourceError)> {
    let (regular, once) = search.split_asked_once();
    if regular.is_empty() {
        return Ok((once.find(request).await?, None));
    }

    let mut findings = Findings::default();
    let mut waiting = regular.kinds(request);
    let mut answers: FuturesUnordered<_> = waiting
        .iter()
        .map(|&kind| {
            let source = regular.only(&[kind]);
            async move { (kind, source.find_for_market(request).await) }
        })
        .collect();
    while let Some((kind, answer)) = answers.next().await {
        waiting.retain(|other| *other != kind);
        match answer {
            Ok(found) => findings.merge(found, request),
            Err(failure) => findings.failed.push(failure),
        }
        tell(&findings, None, &waiting);
    }
    if findings.scanned == 0 && !findings.failed.is_empty() {
        return Err(findings.failed.remove(0));
    }

    // Рынок — только по популярной выдаче: второй проход нарочно собирает дешёвое и занизил бы его.
    let market = market_of(category, request, &findings, new_only);
    match market.and_then(|market| corridor(request, market)) {
        Some((from, to)) => {
            let mut waiting = search.kinds(request);
            waiting.retain(|kind| kind.reads_cheapest_first());
            tell(&findings, market, &waiting);
            let mut answers: FuturesUnordered<_> = waiting
                .iter()
                .map(|&kind| {
                    let source = search.only(&[kind]);
                    async move { (kind, source.find_cheapest(request, from, to).await) }
                })
                .collect();
            while let Some((kind, mut deeper)) = answers.next().await {
                waiting.retain(|other| *other != kind);
                // Площадка уже ответила первым проходом — её предложения в списке, и пугать оговоркой незачем.
                deeper.failed.retain(|(source, _)| source.is_asked_once());
                findings.merge(deeper, request);
                tell(&findings, market, &waiting);
            }
        }
        None if !once.is_empty() => match once.find(request).await {
            Ok(popular) => findings.merge(popular, request),
            Err(failure) => findings.failed.push(failure),
        },
        None => {}
    }
    Ok((findings, market))
}

/// Коридор второго прохода в пределах цены из запроса. Цена за килограмм рублями за пачку не ограничивается,
/// а если всё заказанное дешевле половины рынка, искать в коридоре нечего.
fn corridor(request: &SearchRequest, market: Market) -> Option<(u64, u64)> {
    let from = (market.median / 2).max(request.price.min.unwrap_or(0));
    let to = market.median.min(request.price.max.unwrap_or(u64::MAX));
    (request.per_unit.is_none() && from <= to).then_some((from, to))
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

/// Отбор тот же, что у слежек: иначе самым дешёвым «товаром» всегда окажется чехол к нему.
/// `market` — рынок, если он уже известен; иначе считается по тому, что найдено.
fn priced(
    category: Category,
    request: &SearchRequest,
    findings: &Findings,
    new_only: bool,
    market: Option<Market>,
) -> BestPrices {
    let scanned = findings.scanned;
    let failed = findings
        .failed
        .iter()
        .map(|(source, error)| (*source, failure_reason(*source, error)))
        .collect();

    // Пока идёт поиск, в находках есть и то, что нужно только для рынка.
    let wanted = OfferFilter::new(request);
    let mut seen = HashSet::new();
    let mut other_product = 0;
    let mut same = Vec::new();
    for found in findings.suitable.iter().filter(|found| wanted.accepts(&found.offer)) {
        if !category.accepts(&request.query, &found.offer) {
            other_product += 1;
            continue;
        }
        let used = looks_used(&found.offer);
        let unwanted = new_only && used;
        if !unwanted && !is_copy(&mut seen, &found.offer) {
            same.push((found.source, found.offer.clone(), used));
        }
    }

    let prices: Vec<u64> = same.iter().map(|(_, offer, _)| offer.price).collect();
    let market = market.or_else(|| Market::of(&prices));
    let usual = usual_model_prices(category, request, findings);
    let many_models = model_key(&request.query).is_none();
    let offers: Vec<PricedOffer> = same
        .into_iter()
        .map(|(source, offer, used)| PricedOffer {
            suspicious: price_to_compare(&offer, &usual, market, many_models)
                .is_some_and(|usual| suspiciously_cheap(usual, offer.price)),
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

/// Обычная цена каждой модели в магазинах. На широком запросе рынок один на все модели, и копия дорогой модели за 4 500 ₽
/// выглядит честной ценой рядом с дешёвыми моделями (WH-1000XM6 при настоящих 25 000 ₽, 05.11.2026). Берётся верхняя
/// из двух средних цен: копий бывает столько же, сколько настоящих. Выброс вверх на неё не влияет, поэтому честная
/// цена рядом с одним перекупом подозрительной не становится. Считается по всем находкам, а не только по заказанным:
/// при «до 5000» настоящие цены дорогой модели нужны, чтобы увидеть копию.
fn usual_model_prices(
    category: Category,
    request: &SearchRequest,
    findings: &Findings,
) -> HashMap<BTreeSet<String>, u64> {
    let mut prices: HashMap<BTreeSet<String>, Vec<u64>> = HashMap::new();
    let shop_offers = findings
        .suitable
        .iter()
        .filter(|found| !found.source.has_used_goods() && !looks_used(&found.offer))
        .filter(|found| category.accepts(&request.query, &found.offer));
    for found in shop_offers {
        if let Some(key) = model_key(&found.offer.name) {
            prices.entry(key).or_default().push(found.offer.price);
        }
    }
    prices
        .into_iter()
        .filter(|(_, prices)| prices.len() >= OFFERS_TO_KNOW_MODEL_PRICE)
        .map(|(key, mut prices)| {
            prices.sort_unstable();
            (key, prices[prices.len() / 2])
        })
        .collect()
}

/// С чем сравнить цену предложения. Запрос без номера модели собирает разные модели, и их общий рынок о редкой модели
/// ничего не говорит: честные MDR-EX15AP за 704 ₽ при рынке 8 600 ₽ (05.11.2026) не подозрительны, их просто не с чем сравнить.
fn price_to_compare(
    offer: &Offer,
    usual: &HashMap<BTreeSet<String>, u64>,
    market: Option<Market>,
    many_models: bool,
) -> Option<u64> {
    let market = market.map(|market| market.median);
    match model_key(&offer.name) {
        Some(key) => usual.get(&key).copied().or(market.filter(|_| !many_models)),
        None => market,
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
            suspicious: false,
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

    fn best_prices(category: Category, request: &SearchRequest, findings: &Findings, new_only: bool) -> BestPrices {
        priced(category, request, findings, new_only, None)
    }

    fn popular_and_hidden() -> FakeSource {
        FakeSource::returning(sony(&[3_900, 4_000, 4_100, 4_200, 4_400])).with_cheapest(sony(&[2_500]))
    }

    async fn offers_for(search: &OfferSearch, text: &str) -> Vec<u64> {
        let findings = hunt_offers(search, &SearchRequest::parse(text), |_| {})
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));
        findings.suitable.iter().map(|found| found.offer.price).collect()
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

    /// Отвечает только после того, как окно получило ответ другой площадки.
    struct Slow {
        released: Arc<tokio::sync::Notify>,
    }

    #[async_trait::async_trait]
    impl crate::marketplace::OfferSource for Slow {
        fn name(&self) -> &'static str {
            "медленная"
        }

        async fn search(&self, _query: &str, _scope: crate::marketplace::Scope) -> Result<Vec<Offer>, SourceError> {
            self.released.notified().await;
            Ok(sony(&[3_000]))
        }

        async fn search_link(
            &self,
            _link: &reqwest::Url,
            _scope: crate::marketplace::Scope,
        ) -> Result<Vec<Offer>, SourceError> {
            Ok(Vec::new())
        }

        async fn product(&self, _link: &reqwest::Url) -> Result<crate::marketplace::Product, SourceError> {
            Err(SourceError::NotReady)
        }
    }

    #[tokio::test]
    async fn fast_source_is_shown_while_the_slow_one_still_thinks() {
        let released = Arc::new(tokio::sync::Notify::new());
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(FakeSource::returning(sony(&[4_000])))),
            (
                SourceKind::Wildberries,
                Arc::new(Slow {
                    released: Arc::clone(&released),
                }),
            ),
        ]);
        let seen = std::sync::Mutex::new(Vec::new());

        let prices = hunt_best_prices(&search, &request(), Category::Electronics, false, &|progress| {
            seen.lock()
                .unwrap()
                .push((progress.prices.offers.len(), progress.waiting));
            released.notify_one();
        })
        .await
        .unwrap_or_else(|_| panic!("все площадки молчат"));

        assert_eq!(prices.offers.len(), 2);
        assert_eq!(
            seen.into_inner().unwrap(),
            [(1, vec![SourceKind::Wildberries]), (2, Vec::new())],
            "сначала быстрая площадка и кого ждём, потом обе"
        );
    }

    #[tokio::test]
    async fn second_pass_does_not_wait_for_a_source_that_cannot_sort() {
        let dns = FakeSource::returning(sony(&[3_900, 4_000, 4_100, 4_200, 4_400])).with_cheapest(sony(&[2_500]));
        let asked = dns.queries();
        let search = OfferSearch::new(vec![(SourceKind::Dns, Arc::new(dns))]);

        hunted(&search).await;

        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    async fn hunted(search: &OfferSearch) -> BestPrices {
        hunt_best_prices(search, &request(), Category::Electronics, false, &|_| {})
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
    async fn bot_gets_the_cheapest_hidden_behind_popular_offers_on_top_of_the_whole_list() {
        let source = popular_and_hidden();
        let asked = source.queries();
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(source))]);

        let prices = offers_for(&search, "наушники sony wh-ch520").await;

        assert_eq!(prices, [2_500, 3_900, 4_000, 4_100, 4_200, 4_400]);
        assert_eq!(
            *asked.lock().unwrap(),
            ["наушники sony wh-ch520", "наушники sony wh-ch520 2050..4100"]
        );
    }

    #[tokio::test]
    async fn bot_hears_who_it_waits_for_and_counts_only_what_was_asked() {
        let ozon = popular_and_hidden();
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(ozon))]);
        let request = SearchRequest::parse("наушники sony wh-ch520 до 3000");
        let steps = std::sync::Mutex::new(Vec::new());

        let answer = hunt_offers(&search, &request, |step| steps.lock().unwrap().push(step)).await;

        assert!(answer.is_ok());
        let step = |suitable, waiting: &[SourceKind], deeper| OffersProgress {
            suitable,
            waiting: waiting.to_vec(),
            deeper,
        };
        assert_eq!(
            steps.into_inner().unwrap(),
            [
                step(0, &[], false),
                step(0, &[SourceKind::Ozon], true),
                step(1, &[], true)
            ]
        );
    }

    #[tokio::test]
    async fn price_limit_below_the_market_still_finds_the_market_and_narrows_the_corridor() {
        let source = popular_and_hidden();
        let asked = source.queries();
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(source))]);

        let prices = offers_for(&search, "наушники sony wh-ch520 до 3000").await;

        assert_eq!(prices, [2_500], "популярное дороже 3000 нужно рынку, но не ответу");
        assert_eq!(asked.lock().unwrap()[1], "наушники sony wh-ch520 2050..3000");
    }

    #[tokio::test]
    async fn window_shows_only_what_was_asked_while_the_market_is_counted_over_everything() {
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(popular_and_hidden()))]);
        let shown = std::sync::Mutex::new(Vec::new());
        let request = SearchRequest::parse("наушники sony wh-ch520 до 3000");

        let prices = hunt_best_prices(&search, &request, Category::Electronics, false, &|progress| {
            shown.lock().unwrap().push(progress.prices.offers.len());
        })
        .await
        .unwrap_or_else(|_| panic!("все площадки молчат"));

        assert_eq!(prices.market.unwrap().median, 4_100);
        assert_eq!(prices.best().unwrap().offer.price, 2_500);
        assert_eq!(shown.into_inner().unwrap(), [0, 0, 1]);
    }

    #[tokio::test]
    async fn limit_far_below_the_market_leaves_nothing_to_look_for_in_the_corridor() {
        let source = popular_and_hidden();
        let asked = source.queries();
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(source))]);

        let prices = offers_for(&search, "наушники sony wh-ch520 до 1000").await;

        assert!(prices.is_empty());
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn asking_for_a_discount_or_a_price_per_kilo_does_not_go_for_the_cheapest() {
        for text in ["наушники sony wh-ch520 по скидке", "наушники sony wh-ch520 за кг"] {
            let source = popular_and_hidden();
            let asked = source.queries();
            let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(source))]);

            offers_for(&search, text).await;

            assert_eq!(asked.lock().unwrap().len(), 1, "{text}");
        }
    }

    #[tokio::test]
    async fn price_per_kilo_in_the_window_has_no_corridor_in_rubles_per_pack() {
        let source = popular_and_hidden();
        let asked = source.queries();
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(source))]);
        let request = SearchRequest::parse("наушники sony wh-ch520 за кг");

        let found = hunt_best_prices(&search, &request, Category::Any, false, &|_| {}).await;

        assert!(found.is_ok());
        assert_eq!(asked.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn only_ozon_keeps_to_its_filter_after_both_passes() {
        let sold_by_ozon = |price| Offer {
            sold_by_ozon: true,
            ..sony(&[price]).remove(0)
        };
        let source = FakeSource::returning(sony(&[3_900, 4_000, 4_100, 4_200, 4_400]))
            .with_cheapest(vec![sold_by_ozon(2_600), sony(&[2_500]).remove(0)]);
        let wildberries = FakeSource::returning(Vec::new());
        let visits = wildberries.queries();
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(source)),
            (SourceKind::Wildberries, Arc::new(wildberries)),
        ]);

        let prices = offers_for(&search, "наушники sony wh-ch520 только ozon").await;

        assert_eq!(prices, [2_600]);
        assert!(visits.lock().unwrap().is_empty());
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

    fn grouped(offers: Vec<Finding>) -> Vec<Vec<u64>> {
        let prices = best_prices(Category::Any, &SearchRequest::parse("sony"), &findings(offers), false);
        prices
            .groups()
            .iter()
            .map(|group| group.iter().map(|found| found.offer.price).collect())
            .collect()
    }

    #[test]
    fn one_model_from_different_sources_stands_together_at_the_place_of_the_cheapest() {
        let groups = grouped(vec![
            found(SourceKind::Wildberries, "Наушники Sony WH-CH520 черные", 3_000),
            found(SourceKind::Ozon, "Наушники Sony WH-CH720", 3_100),
            found(SourceKind::Ozon, "SONY наушники WH-CH520, синие", 3_200),
            found(SourceKind::Dns, "Наушники Sony", 3_300),
        ]);

        assert_eq!(groups, [vec![3_000, 3_200], vec![3_100], vec![3_300]]);
    }

    #[test]
    fn twice_the_price_under_the_same_name_is_not_glued() {
        let groups = grouped(vec![
            found(SourceKind::Ozon, "Наушники Sony WH-CH520", 1_000),
            found(SourceKind::Ozon, "Наушники Sony WH-CH520 белые", 2_001),
            found(SourceKind::Wildberries, "Наушники Sony WH-CH520", 2_100),
        ]);

        assert_eq!(groups, [vec![1_000], vec![2_001, 2_100]]);
    }

    fn broad_query(offers: Vec<Finding>) -> BestPrices {
        best_prices(Category::Any, &SearchRequest::parse("sony"), &findings(offers), false)
    }

    fn cheap_models_around_a_copy() -> Vec<Finding> {
        vec![
            found(SourceKind::Ozon, "Наушники Sony WI-C100", 4_300),
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM6", 4_500),
            found(SourceKind::Ozon, "Наушники Sony WI-C310", 4_600),
            found(SourceKind::Ozon, "Наушники Sony WH-CH520", 4_700),
            found(SourceKind::Ozon, "Наушники Sony WI-XB400", 4_800),
            found(SourceKind::Wildberries, "Наушники Sony WH-1000XM6", 25_000),
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM6 серые", 30_000),
        ]
    }

    #[test]
    fn copy_of_an_expensive_model_is_suspicious_though_it_looks_fine_for_the_market() {
        let prices = broad_query(cheap_models_around_a_copy());

        let suspicious: Vec<u64> = prices
            .offers
            .iter()
            .filter(|found| found.suspicious)
            .map(|found| found.offer.price)
            .collect();
        assert_eq!(prices.market.unwrap().median, 4_700);
        assert_eq!(suspicious, [4_500]);
    }

    #[test]
    fn copy_is_not_the_lowest_price_of_its_source_nor_the_best_one() {
        let mut offers = cheap_models_around_a_copy();
        offers.remove(0);

        let prices = broad_query(offers);

        assert_eq!(prices.best().unwrap().offer.price, 4_600);
        assert!(prices.lowest.contains(&(SourceKind::Ozon, 4_600)));
    }

    #[test]
    fn cheap_model_is_judged_by_its_own_price_and_not_by_the_market_of_dearer_ones() {
        let prices = broad_query(vec![
            found(SourceKind::Ozon, "Наушники Sony WH-CH520", 4_300),
            found(SourceKind::Wildberries, "Наушники Sony WH-CH520", 4_400),
            found(SourceKind::Dns, "Наушники Sony WH-CH520", 4_500),
            found(SourceKind::Ozon, "Наушники Sony WF-C710N", 9_000),
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM5", 18_000),
            found(SourceKind::Wildberries, "Наушники Sony WH-1000XM6", 25_000),
            found(SourceKind::Dns, "Наушники Sony WH-1000XM4", 26_000),
        ]);

        assert_eq!(prices.market.unwrap().median, 9_000);
        assert!(prices.offers.iter().all(|found| !found.suspicious));
    }

    #[test]
    fn real_prices_above_the_limit_of_the_request_still_expose_a_copy() {
        let offers = vec![
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM6", 2_500),
            found(SourceKind::Wildberries, "Наушники Sony WH-1000XM6", 25_000),
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM6 серые", 26_000),
            found(SourceKind::Dns, "Наушники Sony WH-1000XM6", 27_000),
        ];
        let request = SearchRequest::parse("наушники sony до 5000");

        let prices = best_prices(Category::Any, &request, &findings(offers), false);

        assert_eq!(prices.offers.len(), 1);
        assert!(prices.offers[0].suspicious);
    }

    fn copies_and_real_ones() -> FakeSource {
        let named = |price, name: &str| Offer {
            url: format!("https://example.com/{price}"),
            ..Offer::sample(name, price)
        };
        FakeSource::returning(vec![
            named(2_500, "Наушники Sony WH-1000XM6"),
            named(25_000, "Наушники Sony WH-1000XM6"),
            named(26_000, "Наушники Sony WH-1000XM6 серые"),
            named(27_000, "Наушники Sony WH-1000XM6 черные"),
            named(4_000, "Наушники Sony WH-CH520"),
        ])
    }

    async fn suspicious_in_answer(text: &str) -> Vec<(u64, bool)> {
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(copies_and_real_ones()))]);
        let findings = hunt_offers(&search, &SearchRequest::parse(text), |_| {})
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));
        findings
            .suitable
            .iter()
            .map(|found| (found.offer.price, found.suspicious))
            .collect()
    }

    #[tokio::test]
    async fn bot_marks_a_copy_and_leaves_honest_offers_alone() {
        let answer = suspicious_in_answer("наушники sony").await;

        assert_eq!(answer[..2], [(2_500, true), (4_000, false)]);
    }

    #[tokio::test]
    async fn bot_marks_a_copy_even_when_real_prices_are_above_the_limit() {
        assert_eq!(
            suspicious_in_answer("наушники sony до 5000").await,
            [(2_500, true), (4_000, false)]
        );
    }

    async fn names_in_answer(text: &str, offers: &[(&str, u64)]) -> Vec<String> {
        let offers = offers
            .iter()
            .map(|&(name, price)| Offer {
                url: format!("https://example.com/{price}"),
                ..Offer::sample(name, price)
            })
            .collect();
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(FakeSource::returning(offers)))]);
        let findings = hunt_offers(&search, &SearchRequest::parse(text), |_| {})
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));
        findings.suitable.into_iter().map(|found| found.offer.name).collect()
    }

    #[tokio::test]
    async fn things_for_the_product_and_defects_leave_the_answer_of_the_bot() {
        let answer = names_in_answer(
            "наушники sony wh-ch520",
            &[
                ("Амбушюры для наушников Sony WH-CH520", 500),
                ("Защитная сумка для наушников Sony WH-CH520", 2_100),
                ("Наушники Sony WH-CH520 на запчасти", 2_200),
                ("Наушники Sony WH-CH520", 4_000),
            ],
        )
        .await;

        assert_eq!(answer, ["Наушники Sony WH-CH520"]);
    }

    #[tokio::test]
    async fn the_same_goes_for_an_answer_by_discount() {
        let answer = names_in_answer(
            "наушники sony по скидке",
            &[("Чехол для наушников Sony", 500), ("Наушники Sony", 4_000)],
        )
        .await;

        assert_eq!(answer, ["Наушники Sony"]);
    }

    #[tokio::test]
    async fn asked_for_a_case_gets_the_case_and_goods_for_someone_stay() {
        let cases = names_in_answer("чехол iphone", &[("Чехол для iPhone 15", 500)]).await;
        let food = names_in_answer("корм для кошек", &[("Сухой корм для кошек Royal Canin", 2_000)]).await;

        assert_eq!(cases, ["Чехол для iPhone 15"]);
        assert_eq!(food, ["Сухой корм для кошек Royal Canin"]);
    }

    #[tokio::test]
    async fn accessories_still_count_as_seen_but_not_as_the_market() {
        let source = FakeSource::returning(vec![
            Offer {
                url: "https://example.com/case".into(),
                ..Offer::sample("Чехол для наушников Sony", 300)
            },
            Offer {
                url: "https://example.com/sony".into(),
                ..Offer::sample("Наушники Sony", 4_000)
            },
        ]);
        let search = OfferSearch::new(vec![(SourceKind::Ozon, Arc::new(source))]);

        let findings = hunt_offers(&search, &SearchRequest::parse("наушники sony"), |_| {})
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));

        assert_eq!((findings.scanned, findings.suitable.len()), (2, 1));
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живые Ozon и Wildberries: cargo test -- --ignored --test-threads=1 --nocapture"]
    async fn goods_for_someone_survive_on_live_sites() {
        use crate::marketplace::ozon::Ozon;
        use crate::marketplace::wildberries::Wildberries;

        let browser = crate::browser::test_browser();
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(Ozon::new(Arc::clone(&browser), 3))),
            (SourceKind::Wildberries, Arc::new(Wildberries::new(browser))),
        ]);
        let request = SearchRequest::parse("корм для кошек royal canin");

        let all = search
            .find(&request)
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));
        let names: Vec<String> = all.suitable.iter().map(|found| found.offer.name.clone()).collect();
        let dropped: Vec<&String> = all
            .suitable
            .iter()
            .filter(|found| !Category::Any.accepts(&request.query, &found.offer))
            .map(|found| &found.offer.name)
            .collect();

        println!("подошло {}, ушло бы {}: {dropped:#?}", names.len(), dropped.len());
        assert!(dropped.len() * 2 < names.len());
    }

    #[tokio::test]
    async fn small_pack_next_to_big_ones_is_not_a_copy() {
        let packs = [
            ("Порошок Persil 450 г", 180),
            ("Порошок Persil 3 кг", 750),
            ("Порошок Persil 4 кг", 900),
            ("Порошок Persil 5 кг", 1_100),
            ("Порошок Persil 6 кг", 1_400),
        ]
        .map(|(name, price)| Offer {
            url: format!("https://example.com/{price}"),
            ..Offer::sample(name, price)
        });
        let search = OfferSearch::new(vec![(
            SourceKind::Ozon,
            Arc::new(FakeSource::returning(packs.to_vec())),
        )]);

        let findings = hunt_offers(&search, &SearchRequest::parse("persil"), |_| {})
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));

        assert_eq!(findings.suitable.len(), 5);
        assert!(findings.suitable.iter().all(|found| !found.suspicious));
    }

    #[tokio::test]
    async fn bot_marks_a_copy_in_an_answer_by_discount_too() {
        let answer = suspicious_in_answer("наушники sony по скидке до 5000").await;

        assert!(answer.contains(&(2_500, true)), "{answer:?}");
    }

    #[test]
    fn rare_model_on_a_broad_query_has_nothing_to_be_compared_with() {
        let mut offers = cheap_models_around_a_copy();
        offers.insert(0, found(SourceKind::Wildberries, "Наушники Sony MDR-EX15AP", 700));

        let prices = broad_query(offers);

        assert_eq!(prices.best().unwrap().offer.price, 700);
    }

    #[test]
    fn query_for_one_model_still_compares_a_rare_title_with_the_market() {
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Ozon, "Наушники Sony WH-CH520 EU1", 900));

        let prices = best_prices(Category::Electronics, &request(), &findings(offers), false);

        assert!(prices.offers[0].suspicious);
    }

    #[test]
    fn title_without_a_model_number_is_still_compared_with_the_market() {
        let mut offers = cheap_models_around_a_copy();
        offers.insert(0, found(SourceKind::Ozon, "Наушники Sony", 700));

        let prices = broad_query(offers);

        assert!(prices.offers[0].suspicious);
    }

    #[test]
    fn single_overpriced_seller_does_not_make_honest_prices_suspicious() {
        let prices = broad_query(vec![
            found(SourceKind::Ozon, "Наушники Sony WH-CH520", 3_000),
            found(SourceKind::Wildberries, "Наушники Sony WH-CH520", 3_100),
            found(SourceKind::Dns, "Наушники Sony WH-CH520", 9_000),
        ]);

        assert!(prices.offers.iter().all(|found| !found.suspicious));
    }

    #[test]
    fn two_prices_of_a_model_do_not_tell_which_is_usual() {
        let prices = broad_query(vec![
            found(SourceKind::Ozon, "Наушники Sony WH-CH520", 3_000),
            found(SourceKind::Wildberries, "Наушники Sony WH-CH520", 9_000),
        ]);

        assert!(prices.offers.iter().all(|found| !found.suspicious));
    }

    #[test]
    fn cheap_ad_is_not_compared_with_the_model_and_does_not_drag_its_price() {
        let prices = broad_query(vec![
            found(SourceKind::Avito, "Наушники Sony WH-1000XM6", 2_000),
            found(SourceKind::Avito, "Наушники Sony WH-1000XM6", 2_100),
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM6", 4_000),
            found(SourceKind::Wildberries, "Наушники Sony WH-1000XM6", 25_000),
            found(SourceKind::Dns, "Наушники Sony WH-1000XM6", 26_000),
        ]);

        let ozon = prices
            .offers
            .iter()
            .find(|found| found.source == SourceKind::Ozon)
            .unwrap();
        assert!(ozon.suspicious, "обычная цена — по магазинам, а не по объявлениям");
    }

    #[test]
    fn copies_of_a_model_stand_together_apart_from_the_real_ones() {
        let groups = grouped(vec![
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM6", 4_500),
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM6 черные", 4_600),
            found(SourceKind::Wildberries, "Наушники Sony WH-1000XM6", 25_000),
            found(SourceKind::Ozon, "Наушники Sony WH-1000XM6 серые", 26_000),
            found(SourceKind::Dns, "Наушники Sony WH-1000XM6", 27_000),
        ]);

        assert_eq!(groups, [vec![4_500, 4_600], vec![25_000, 26_000, 27_000]]);
    }

    #[test]
    fn ads_used_and_suspicious_stay_apart_from_the_model() {
        let shop = "Наушники Sony WH-CH520";
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, shop, 3_000));
        offers.insert(1, found(SourceKind::Ozon, "Наушники Sony WH-CH520 б/у", 3_100));
        offers.insert(0, found(SourceKind::Ozon, shop, 900));

        let prices = best_prices(Category::Electronics, &request(), &findings(offers), false);
        let groups = prices.groups();

        let sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
        assert_eq!(sizes, [1, 1, 1, 5]);
    }

    #[test]
    fn best_price_comes_from_all_sources_and_each_shows_its_lowest() {
        let prices = best_prices(Category::Electronics, &request(), &findings(market_of_five()), false);

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

        let prices = best_prices(Category::Electronics, &request(), &findings(offers), false);

        assert_eq!(prices.best().unwrap().offer.price, 3_900);
        assert_eq!(prices.other_product, 1);
        assert_eq!(prices.scanned, 6);
    }

    #[test]
    fn suspiciously_cheap_is_shown_but_is_not_the_best_price() {
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, "Наушники Sony WH-CH520", 900));

        let prices = best_prices(Category::Electronics, &request(), &findings(offers), false);

        assert!(prices.offers[0].suspicious);
        assert_eq!(prices.best().unwrap().offer.price, 3_900);
        assert!(!prices.lowest.iter().any(|(source, _)| *source == SourceKind::Avito));
    }

    #[test]
    fn ad_without_a_word_about_condition_is_not_the_best_shop_price() {
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, "Наушники Sony WH-CH520", 2_500));

        let prices = best_prices(Category::Electronics, &request(), &findings(offers), false);

        assert_eq!(prices.best().unwrap().offer.price, 3_900);
        assert_eq!(prices.best_ad().unwrap().offer.price, 2_500);
    }

    #[test]
    fn used_goes_apart_from_the_best_shop_price_or_away_when_asked() {
        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, "Наушники Sony WH-CH520 б/у", 2_500));

        let prices = best_prices(Category::Electronics, &request(), &findings(offers), false);
        assert_eq!(prices.best().unwrap().offer.price, 3_900);
        assert_eq!(prices.best_ad().unwrap().offer.price, 2_500);

        let mut offers = market_of_five();
        offers.insert(0, found(SourceKind::Avito, "Наушники Sony WH-CH520 б/у", 2_500));
        let new_only = best_prices(Category::Electronics, &request(), &findings(offers), true);
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
            &findings(vec![copy(1), copy(2)]),
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

        let prices = best_prices(Category::Electronics, &request(), &answer, false);

        assert_eq!(prices.failed.len(), 1);
        assert!(prices.failed[0].1.contains("не робот"), "{}", prices.failed[0].1);
    }

    #[test]
    fn few_offers_give_no_market_and_nothing_is_suspicious() {
        let offers = vec![found(SourceKind::Ozon, "Наушники Sony WH-CH520", 900)];

        let prices = best_prices(Category::Electronics, &request(), &findings(offers), false);

        assert!(prices.market.is_none());
        assert_eq!(prices.best().unwrap().offer.price, 900);
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живые Ozon, Wildberries и ДНС: cargo test -- --ignored --test-threads=1"]
    async fn bot_looks_below_the_market_on_live_sites() {
        use crate::marketplace::dns::Dns;
        use crate::marketplace::ozon::Ozon;
        use crate::marketplace::wildberries::Wildberries;

        let browser = crate::browser::test_browser();
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(Ozon::new(Arc::clone(&browser), 3))),
            (
                SourceKind::Wildberries,
                Arc::new(Wildberries::new(Arc::clone(&browser))),
            ),
            (SourceKind::Dns, Arc::new(Dns::new(browser))),
        ]);
        let request = SearchRequest::parse("наушники sony wh-ch520 до 3500");
        let started = std::time::Instant::now();

        let findings = hunt_offers(&search, &request, |step| println!("{step:?}"))
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));

        println!(
            "за {:?}: просмотрено {}, подошло {}, не ответили {:?}",
            started.elapsed(),
            findings.scanned,
            findings.suitable.len(),
            findings.failed
        );
        for found in findings.suitable.iter().take(12) {
            let mark = if found.suspicious {
                "подозрительно"
            } else {
                ""
            };
            println!("{:?} {} ₽ {} {mark}", found.source, found.offer.price, found.offer.name);
        }
        assert!(!findings.suitable.is_empty());
        assert!(findings.suitable.iter().all(|found| found.offer.price <= 3_500));
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живые Ozon, Wildberries и ДНС: cargo test -- --ignored --test-threads=1 --nocapture"]
    async fn models_glued_on_live_sites_can_be_read_by_eye() {
        use crate::marketplace::dns::Dns;
        use crate::marketplace::ozon::Ozon;
        use crate::marketplace::wildberries::Wildberries;

        let browser = crate::browser::test_browser();
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(Ozon::new(Arc::clone(&browser), 3))),
            (
                SourceKind::Wildberries,
                Arc::new(Wildberries::new(Arc::clone(&browser))),
            ),
            (SourceKind::Dns, Arc::new(Dns::new(browser))),
        ]);
        let request = SearchRequest::parse("наушники sony");

        let prices = hunt_best_prices(&search, &request, Category::Electronics, false, &|_| {})
            .await
            .unwrap_or_else(|_| panic!("все площадки молчат"));

        let groups = prices.groups();
        println!("предложений {}, в списке {}", prices.offers.len(), groups.len());
        for group in groups.iter().filter(|group| group.len() > 1 || group[0].suspicious) {
            println!(
                "---{}",
                if group[0].suspicious {
                    " подозрительно"
                } else {
                    ""
                }
            );
            for found in group {
                println!("{:?} {} ₽ {}", found.source, found.offer.price, found.offer.name);
            }
        }
        println!("лучшая цена: {:?}", prices.best().map(|found| &found.offer.name));
        assert!(groups.len() < prices.offers.len());
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

        let prices = hunt_best_prices(&search, &request, Category::Electronics, false, &|_| {})
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
