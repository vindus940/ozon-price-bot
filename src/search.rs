mod filter;
mod order;
mod request;
mod synonyms;

use std::sync::Arc;

use crate::marketplace::{OfferSource, SourceError};
use crate::offer::Offer;
use crate::quantity::{Quantity, Unit, UnitPrice, most_common_unit};
pub use filter::OfferFilter;
pub use order::Order;
pub use request::{OzonConditions, PriceRange, SearchRequest};

pub struct OfferSearch {
    source: Arc<dyn OfferSource>,
}

pub struct Findings {
    pub scanned: usize,
    /// Подходящие под запрос в заказанном порядке (по умолчанию от дешёвых к дорогим). Если просили цену
    /// за единицу — сначала те, у кого фасовка в этих единицах, и сравниваются они по цене за единицу.
    pub suitable: Vec<Finding>,
    /// В чём фасовано большинство подходящих: повод подсказать сравнение «за кг», если его не просили.
    pub packed_in: Option<Unit>,
}

/// Цена за единицу — вывод поиска, а не данные площадки, поэтому она рядом с предложением, а не в нём.
pub struct Finding {
    pub offer: Offer,
    /// Только в единицах из запроса: у геля в литрах при поиске «за кг» её нет.
    pub unit_price: Option<UnitPrice>,
}

impl OfferSearch {
    pub fn new(source: Arc<dyn OfferSource>) -> Self {
        Self { source }
    }

    pub fn source_name(&self) -> &'static str {
        self.source.name()
    }

    pub async fn find(&self, request: &SearchRequest) -> Result<Findings, SourceError> {
        let offers = self.source.search(&request.query, None).await?;
        let scanned = offers.len();
        let filter = OfferFilter::new(request);
        let measured: Vec<(Offer, Option<Quantity>)> = offers
            .into_iter()
            .filter(|offer| filter.accepts(offer))
            .map(|offer| {
                let quantity = Quantity::in_title(&offer.name);
                (offer, quantity)
            })
            .collect();
        let packed_in = most_common_unit(measured.iter().map(|(_, quantity)| *quantity))
            .filter(|&(_, count)| count * 2 > measured.len())
            .map(|(unit, _)| unit);

        let suitable: Vec<Finding> = measured
            .into_iter()
            .map(|(offer, quantity)| Finding {
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marketplace::testing::FakeSource;

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
        let search = OfferSearch::new(Arc::new(source));

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
        OfferSearch::new(Arc::new(FakeSource::returning(vec![
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
        let search = OfferSearch::new(Arc::new(ozon));

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
    #[ignore = "открывает браузер и ходит на живой Ozon: cargo test -- --ignored --test-threads=1"]
    async fn orders_make_sense_on_live_site() {
        let ozon = crate::marketplace::ozon::Ozon::new(crate::browser::test_browser(), 3);
        let offers = ozon.search("наушники sony", None).await.unwrap();
        // Одна выдача на все три порядка: так сравнивается порядок, а не разные ответы Ozon.
        let search = OfferSearch::new(Arc::new(FakeSource::returning(offers)));
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
