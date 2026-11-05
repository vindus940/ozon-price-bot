mod filter;
mod order;
mod request;
mod synonyms;

use std::sync::Arc;

use futures_util::future::join_all;

use crate::marketplace::{OfferSource, Scope, SourceError, SourceKind};
use crate::offer::Offer;
use crate::quantity::{Quantity, Unit, UnitPrice, most_common_unit};
pub use filter::{OfferFilter, first_mention, mentions};
pub use order::Order;
pub use request::{PriceRange, SearchRequest, ShopConditions};

/// Меньше предложений — самая высокая цена выдачи ни о чём не говорит.
const OFFERS_TO_JUDGE_OLD_PRICE: usize = 5;

/// Разовый поиск по нескольким площадкам сразу: лучшая цена — по всем, а не по одному магазину.
pub struct OfferSearch {
    sources: Vec<(SourceKind, Arc<dyn OfferSource>)>,
}

pub struct Findings {
    pub scanned: usize,
    /// Подходящие под запрос в заказанном порядке (по умолчанию от дешёвых к дорогим). Если просили цену
    /// за единицу — сначала те, у кого фасовка в этих единицах, и сравниваются они по цене за единицу.
    pub suitable: Vec<Finding>,
    /// В чём фасовано большинство подходящих: повод подсказать сравнение «за кг», если его не просили.
    pub packed_in: Option<Unit>,
    /// Площадки, которые не ответили, хотя другие ответили: их предложений в списке нет, и об этом надо сказать.
    pub failed: Vec<(SourceKind, SourceError)>,
}

/// Цена за единицу и площадка — вывод поиска, а не данные предложения, поэтому они рядом с ним, а не в нём.
pub struct Finding {
    pub source: SourceKind,
    pub offer: Offer,
    /// Только в единицах из запроса: у геля в литрах при поиске «за кг» её нет.
    pub unit_price: Option<UnitPrice>,
}

impl OfferSearch {
    pub fn new(sources: Vec<(SourceKind, Arc<dyn OfferSource>)>) -> Self {
        Self { sources }
    }

    /// Куда идёт этот запрос: «только Ozon» другие площадки всё равно не пройдут, и заходить на них незачем.
    fn asked(&self, request: &SearchRequest) -> impl Iterator<Item = &(SourceKind, Arc<dyn OfferSource>)> {
        self.sources
            .iter()
            .filter(|(kind, _)| !request.shop.only_ozon || kind.has_marketplace_seller())
    }

    pub fn source_names(&self, request: &SearchRequest) -> Vec<&'static str> {
        self.asked(request).map(|(kind, _)| kind.title()).collect()
    }

    /// Площадки опрашиваются одновременно, у каждой своя вкладка. Ошибка — только если не нашлось нигде и хоть одна
    /// площадка не ответила; иначе ответ из того, что есть, а не ответившие — в `Findings::failed`.
    pub async fn find(&self, request: &SearchRequest) -> Result<Findings, (SourceKind, SourceError)> {
        let answers = join_all(
            self.asked(request)
                .map(|(kind, source)| async move { (*kind, source.search(&request.query, Scope::default()).await) }),
        )
        .await;
        let mut offers: Vec<(SourceKind, Offer)> = Vec::new();
        let mut failed = Vec::new();
        for (kind, answer) in answers {
            match answer {
                Ok(found) => offers.extend(found.into_iter().map(|offer| (kind, offer))),
                Err(error) => failed.push((kind, error)),
            }
        }
        if offers.is_empty() && !failed.is_empty() {
            return Err(failed.remove(0));
        }

        let scanned = offers.len();
        let filter = OfferFilter::new(request);
        let measured: Vec<(SourceKind, Offer, Option<Quantity>)> = offers
            .into_iter()
            .filter(|(_, offer)| filter.accepts(offer))
            .map(|(kind, offer)| {
                let quantity = Quantity::in_title(&offer.name);
                (kind, offer, quantity)
            })
            .collect();
        let packed_in = most_common_unit(measured.iter().map(|(_, _, quantity)| *quantity))
            .filter(|&(_, count)| count * 2 > measured.len())
            .map(|(unit, _)| unit);

        let suitable: Vec<Finding> = measured
            .into_iter()
            .map(|(source, offer, quantity)| Finding {
                source,
                unit_price: quantity
                    .filter(|quantity| Some(quantity.unit) == request.per_unit)
                    .map(|quantity| quantity.unit_price(offer.price)),
                offer,
            })
            .collect();
        let suitable = request.order.arrange(suitable);
        Ok(Findings {
            scanned,
            suitable,
            packed_in,
            failed,
        })
    }
}

impl Findings {
    /// Выше этого старая цена не похожа на настоящую: в полтора раза дороже самого дорогого, что сейчас продаётся
    /// в той же выдаче. Запас нужен, потому что сама дорогая вещь выдачи могла честно подешеветь.
    pub fn believable_old_price(&self) -> Option<u64> {
        if self.suitable.len() < OFFERS_TO_JUDGE_OLD_PRICE {
            return None;
        }
        let highest = self.suitable.iter().map(|found| found.offer.price).max()?;
        Some(highest + highest / 2)
    }

    /// Заново отобрать и упорядочить уже найденное по изменившемуся запросу, не заходя на площадку.
    pub fn refine(&mut self, request: &SearchRequest) {
        let filter = OfferFilter::new(request);
        let kept = std::mem::take(&mut self.suitable)
            .into_iter()
            .filter(|found| filter.accepts(&found.offer))
            .collect();
        self.suitable = request.order.arrange(kept);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marketplace::testing::FakeSource;

    fn on_ozon(source: Arc<dyn OfferSource>) -> OfferSearch {
        OfferSearch::new(vec![(SourceKind::Ozon, source)])
    }

    fn sourced(findings: &Findings) -> Vec<(SourceKind, u64)> {
        findings
            .suitable
            .iter()
            .map(|found| (found.source, found.offer.price))
            .collect()
    }

    #[tokio::test]
    async fn offers_of_all_sources_make_one_list_with_their_source() {
        let ozon = FakeSource::returning(vec![Offer::sample("Наушники Sony", 3000)]);
        let wildberries = FakeSource::returning(vec![Offer::sample("Наушники Sony", 2500)]);
        let dns = FakeSource::returning(vec![Offer::sample("Наушники Sony", 4000)]);
        let search = OfferSearch::new(vec![
            (SourceKind::Ozon, Arc::new(ozon)),
            (SourceKind::Wildberries, Arc::new(wildberries)),
            (SourceKind::Dns, Arc::new(dns)),
        ]);

        let findings = search.find(&SearchRequest::parse("наушники sony")).await.unwrap();

        assert_eq!(findings.scanned, 3);
        assert_eq!(
            sourced(&findings),
            [
                (SourceKind::Wildberries, 2500),
                (SourceKind::Ozon, 3000),
                (SourceKind::Dns, 4000)
            ]
        );
        assert!(findings.failed.is_empty());
    }

    #[tokio::test]
    async fn one_silent_source_does_not_spoil_the_answer_of_the_others() {
        let search = OfferSearch::new(vec![
            (
                SourceKind::Ozon,
                Arc::new(FakeSource::returning(vec![Offer::sample("Наушники Sony", 3000)])),
            ),
            (SourceKind::Wildberries, Arc::new(FakeSource::asking_captcha())),
        ]);

        let findings = search.find(&SearchRequest::parse("наушники sony")).await.unwrap();

        assert_eq!(sourced(&findings), [(SourceKind::Ozon, 3000)]);
        assert!(matches!(
            findings.failed.as_slice(),
            [(SourceKind::Wildberries, SourceError::Captcha)]
        ));
    }

    #[tokio::test]
    async fn no_answer_from_anywhere_is_an_error_naming_the_source() {
        let search = on_ozon(Arc::new(FakeSource::asking_captcha()));

        let failure = search.find(&SearchRequest::parse("наушники sony")).await;

        assert!(matches!(failure, Err((SourceKind::Ozon, SourceError::Captcha))));
    }

    #[tokio::test]
    async fn old_price_is_judged_only_with_enough_offers_around() {
        let few = on_ozon(Arc::new(FakeSource::returning(vec![Offer::sample(
            "Наушники Sony",
            1000,
        )])));
        let findings = few.find(&SearchRequest::parse("наушники sony")).await.unwrap();
        assert_eq!(findings.believable_old_price(), None);

        let offers = (1..=5)
            .map(|step| Offer::sample("Наушники Sony", step * 1000))
            .collect();
        let many = on_ozon(Arc::new(FakeSource::returning(offers)));
        let findings = many.find(&SearchRequest::parse("наушники sony")).await.unwrap();
        assert_eq!(findings.believable_old_price(), Some(7500));
    }

    #[tokio::test]
    async fn only_ozon_does_not_visit_other_sites() {
        let wildberries = FakeSource::returning(Vec::new());
        let visits = wildberries.queries();
        let search = OfferSearch::new(vec![
            (
                SourceKind::Ozon,
                Arc::new(FakeSource::returning(vec![Offer {
                    sold_by_ozon: true,
                    ..Offer::sample("Наушники Sony", 3000)
                }])),
            ),
            (SourceKind::Wildberries, Arc::new(wildberries)),
        ]);
        let request = SearchRequest::parse("наушники sony только ozon");

        let findings = search.find(&request).await.unwrap();

        assert_eq!(sourced(&findings), [(SourceKind::Ozon, 3000)]);
        assert!(visits.lock().unwrap().is_empty());
        assert_eq!(search.source_names(&request), ["Ozon"]);
    }

    #[tokio::test]
    async fn refine_applies_a_stricter_request_to_what_was_found() {
        let rated = |name: &str, price: u64, rating: f32| Offer {
            rating: Some(rating),
            ..Offer::sample(name, price)
        };
        let search = on_ozon(Arc::new(FakeSource::returning(vec![
            rated("Наушники Sony A", 1000, 4.0),
            rated("Наушники Sony B", 3000, 4.9),
            rated("Наушники Sony C", 2000, 4.6),
        ])));
        let mut findings = search.find(&SearchRequest::parse("наушники sony")).await.unwrap();

        findings.refine(&SearchRequest::parse("наушники sony рейтинг от 4.5"));

        assert_eq!(prices(&findings), [2000, 3000]);
        assert_eq!(findings.scanned, 3);
    }

    fn prices(findings: &Findings) -> Vec<u64> {
        findings.suitable.iter().map(|found| found.offer.price).collect()
    }

    #[tokio::test]
    async fn returns_suitable_offers_cheapest_first() {
        let source = FakeSource::returning(vec![
            Offer::sample("Наушники Sony WH-CH520", 4364),
            Offer::sample("Амбушюры для наушников Sony", 316),
            Offer::sample("Наушники Sony WI-C100", 2467),
            Offer::sample("Колонка Sony", 1990),
            Offer::sample("Наушники Sony WH-1000XM5", 22754),
        ]);
        let queries = source.queries();
        let search = on_ozon(Arc::new(source));

        let findings = search
            .find(&SearchRequest::parse("наушники sony -амбушюры до 5000"))
            .await
            .unwrap();

        assert_eq!(*queries.lock().unwrap(), ["наушники sony"]);
        assert_eq!(findings.scanned, 5);
        assert_eq!(prices(&findings), [2467, 4364]);
        assert_eq!(findings.packed_in, None);
    }

    fn powders() -> OfferSearch {
        on_ozon(Arc::new(FakeSource::returning(vec![
            Offer::sample("Порошок Persil 450 г", 180),
            Offer::sample("Порошок Persil 3 кг", 750),
            Offer::sample("Гель Persil 1 л", 300),
            Offer::sample("Порошок Persil 6 кг", 1400),
            Offer::sample("Порошок Persil", 100),
        ])))
    }

    #[tokio::test]
    async fn per_kilogram_puts_the_cheapest_kilogram_first() {
        let findings = powders().find(&SearchRequest::parse("persil за кг")).await.unwrap();

        assert_eq!(prices(&findings), [1400, 750, 180, 100, 300]);
        let per_kilogram: Vec<Option<u64>> = findings
            .suitable
            .iter()
            .map(|found| found.unit_price.map(|price| price.kopecks / 100))
            .collect();
        assert_eq!(per_kilogram, [Some(233), Some(250), Some(400), None, None]);
    }

    #[tokio::test]
    async fn without_asking_sorts_by_price_and_notices_pack_sizes() {
        let findings = powders().find(&SearchRequest::parse("persil")).await.unwrap();

        assert_eq!(prices(&findings), [100, 180, 300, 750, 1400]);
        assert!(findings.suitable.iter().all(|found| found.unit_price.is_none()));
        assert_eq!(findings.packed_in, Some(Unit::Kilogram));
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Ozon: cargo test -- --ignored --test-threads=1"]
    async fn cheapest_kilogram_first_on_live_site() {
        let ozon = crate::marketplace::ozon::Ozon::new(crate::browser::test_browser(), 3);
        let search = on_ozon(Arc::new(ozon));

        let findings = search
            .find(&SearchRequest::parse("стиральный порошок за кг рейтинг от 4.5"))
            .await
            .unwrap();

        let measured = findings
            .suitable
            .iter()
            .filter(|found| found.unit_price.is_some())
            .count();
        for found in &findings.suitable {
            let unit_price = found.unit_price.map_or("—".into(), |price| price.to_string());
            println!("{unit_price} · {} ₽ · {}", found.offer.price, found.offer.name);
        }
        assert!(
            measured * 2 > findings.suitable.len(),
            "фасовка прочитана только у {measured} из {}",
            findings.suitable.len()
        );
        assert!(
            findings
                .suitable
                .iter()
                .all(|found| found.offer.rating.is_some_and(|r| r >= 4.5))
        );
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живые Ozon, Wildberries и ДНС: cargo test -- --ignored --test-threads=1"]
    async fn three_sites_answer_at_once_in_their_own_tabs() {
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

        let findings = search.find(&SearchRequest::parse("наушники sony")).await.unwrap();

        let from = |source: SourceKind| findings.suitable.iter().filter(|found| found.source == source).count();
        let counts = [SourceKind::Ozon, SourceKind::Wildberries, SourceKind::Dns].map(from);
        println!("Ozon, WB, ДНС: {counts:?}, не ответили: {:?}", findings.failed);
        let ceiling = findings.believable_old_price().unwrap_or(u64::MAX);
        let with_old_price = findings
            .suitable
            .iter()
            .filter(|found| found.offer.old_price.is_some())
            .count();
        println!("старая цена есть у {with_old_price}, предел {ceiling} ₽, выше него:");
        for found in findings
            .suitable
            .iter()
            .filter(|found| found.offer.old_price.is_some_and(|old| old > ceiling))
        {
            println!(
                "  {:?} {} ₽ <- {:?} {}",
                found.source, found.offer.price, found.offer.old_price, found.offer.name
            );
        }
        assert!(findings.failed.is_empty(), "{:?}", findings.failed);
        assert!(counts.iter().all(|&count| count > 0), "{counts:?}");
    }

    #[tokio::test]
    #[ignore = "открывает браузер и ходит на живой Ozon: cargo test -- --ignored --test-threads=1"]
    async fn orders_make_sense_on_live_site() {
        let ozon = crate::marketplace::ozon::Ozon::new(crate::browser::test_browser(), 3);
        let offers = ozon.search("наушники sony", Scope::default()).await.unwrap();
        // Одна выдача на все три порядка: так сравнивается порядок, а не разные ответы Ozon.
        let search = on_ozon(Arc::new(FakeSource::returning(offers)));
        let arranged = |text: &'static str| {
            let search = &search;
            async move { search.find(&SearchRequest::parse(text)).await.unwrap().suitable }
        };

        let cheapest = arranged("наушники sony").await;
        let on_sale = arranged("наушники sony по скидке").await;
        let balanced = arranged("наушники sony цена-качество").await;
        for (title, list) in [
            ("дешёвые", &cheapest),
            ("по скидке", &on_sale),
            ("цена-качество", &balanced),
        ] {
            println!("\n{title}:");
            for found in list.iter().take(5) {
                let offer = &found.offer;
                println!(
                    "{} ₽ · −{}% · ★{:?} · отзывов {:?} · {}",
                    offer.price,
                    offer.discount_percent().unwrap_or(0),
                    offer.rating,
                    offer.reviews,
                    offer.name
                );
            }
        }

        let best_discount = on_sale.iter().filter_map(|found| found.offer.discount_percent()).max();
        assert_eq!(on_sale[0].offer.discount_percent(), best_discount);
        let top = &balanced[0].offer;
        assert!(top.rating.is_some_and(|rating| rating >= 4.5) && top.reviews.is_some_and(|reviews| reviews >= 100));
    }
}
