use std::collections::HashSet;

use chrono::{DateTime, Local};

use crate::hunt::category::looks_used;
use crate::hunt::deal::{Deal, evaluate};
use crate::hunt::market::{Market, MarketPrice, Pricing};
use crate::hunt::watch::Watch;
use crate::offer::Offer;
use crate::search::OfferFilter;

/// Результат одной проверки, ещё не сверенный с памятью об отправленном.
pub struct Assessment {
    pub scanned: usize,
    pub comparable: usize,
    pub market: Option<MarketPrice>,
    /// Выгодные: сначала с наибольшей выгодой перепродажи, потом с наибольшей скидкой.
    pub suitable: Vec<Deal>,
    pub hidden_suspicious: usize,
}

impl Assessment {
    fn without_market(scanned: usize, comparable: usize) -> Self {
        Self {
            scanned,
            comparable,
            market: None,
            suitable: Vec::new(),
            hidden_suspicious: 0,
        }
    }
}

/// Рынок считается по всем похожим предложениям, а выгодные выбираются только из тех,
/// что проходят ваши фильтры: иначе фильтр «до 60 000 ₽» занижал бы и саму рыночную цену.
pub fn assess(watch: &Watch, offers: Vec<Offer>, now: DateTime<Local>) -> Assessment {
    let scanned = offers.len();
    let similar = OfferFilter::new(&watch.comparables_request());
    let comparables = without_copies(offers.into_iter().filter(|offer| {
        similar.accepts(offer) && watch.category.accepts(&watch.query, offer) && !(watch.new_only && looks_used(offer))
    }));
    let Some(pricing) = Pricing::for_offers(watch.category, &comparables) else {
        return Assessment::without_market(scanned, 0);
    };
    let (comparables, prices): (Vec<Offer>, Vec<u64>) = comparables
        .into_iter()
        .filter_map(|offer| {
            let price = pricing.comparable_price(&offer)?;
            Some((offer, price))
        })
        .unzip();
    let comparable = comparables.len();
    let Some(market) = Market::of(&prices) else {
        return Assessment::without_market(scanned, comparable);
    };

    let wanted = OfferFilter::new(&watch.candidates_request());
    let (suspicious, mut suitable): (Vec<Deal>, Vec<Deal>) = comparables
        .into_iter()
        .filter(|offer| wanted.accepts(offer))
        .filter_map(|offer| {
            let (offer_market, per_unit) = pricing.market_for(market, &offer);
            let verdict = evaluate(watch.goal, watch.radius_km, offer_market, &offer)?;
            Some(Deal {
                watch_id: watch.id,
                watch_name: watch.name.clone(),
                found_at: now,
                offer,
                market_price: offer_market.median,
                market_sample: market.sample,
                discount_percent: verdict.discount_percent,
                resale_profit: verdict.resale_profit,
                suspicious: verdict.suspicious,
                per_unit,
            })
        })
        .partition(|deal| deal.suspicious && !watch.include_suspicious);
    suitable.sort_by(|a, b| {
        b.resale_profit
            .cmp(&a.resale_profit)
            .then(b.discount_percent.cmp(&a.discount_percent))
    });

    Assessment {
        scanned,
        comparable,
        market: Some(pricing.market_price(market)),
        suitable,
        hidden_suspicious: suspicious.len(),
    }
}

/// Перекупщики выкладывают один товар пачкой одинаковых объявлений — та же цена, то же место.
/// Такие считаются за одно, иначе один продавец задаёт «рыночную» цену и шлёт пачку одинаковых уведомлений.
fn without_copies(offers: impl Iterator<Item = Offer>) -> Vec<Offer> {
    let mut seen = HashSet::new();
    offers
        .filter(|offer| {
            offer
                .place
                .as_ref()
                .is_none_or(|place| seen.insert((offer.price, place.clone())))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hunt::category::Category;
    use crate::hunt::fixtures::{iphone_watch, market_offers, phone};
    use crate::hunt::watch::Goal;
    use crate::marketplace::SourceKind;
    use crate::search::{PriceRange, ShopConditions};

    #[test]
    fn new_only_compares_new_with_new() {
        let listing = |price: u64, text: &str| Offer {
            url: format!("https://www.avito.ru/{price}"),
            description: Some(text.into()),
            ..Offer::sample("Наушники Logitech G435", price)
        };
        let mut offers: Vec<Offer> = [6_000, 6_200, 6_400, 6_500, 6_800]
            .map(|price| listing(price, "Новые, в упаковке"))
            .to_vec();
        offers.extend([
            listing(1_500, "После покупки пользовался около года"),
            listing(1_700, "Оригинал, состояние 9/10"),
            listing(4_900, "Новые, запечатаны"),
        ]);
        let new_only = Watch {
            query: "logitech g435".into(),
            category: Category::Any,
            source: SourceKind::Avito,
            new_only: true,
            ..iphone_watch()
        };

        let assessment = assess(&new_only, offers, Local::now());

        assert_eq!(assessment.comparable, 6, "б/у не участвуют в рынке новых");
        assert_eq!(assessment.market, Some(MarketPrice::Whole(6_300)));
        assert_eq!(
            assessment.suitable.iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [4_900]
        );
    }

    #[test]
    fn market_ignores_accessories_and_other_versions_and_your_price_filter() {
        let mut offers = market_offers();
        offers.push(phone(55_000));
        let picky = Watch {
            price: PriceRange {
                min: None,
                max: Some(60_000),
            },
            ..iphone_watch()
        };

        let assessment = assess(&picky, offers, Local::now());

        assert_eq!(assessment.scanned, 9);
        assert_eq!(assessment.comparable, 7);
        assert_eq!(assessment.market, Some(MarketPrice::Whole(72_000)));
        assert_eq!(assessment.suitable.len(), 1);
        assert_eq!(assessment.suitable[0].offer.price, 55_000);
        assert_eq!(assessment.suitable[0].discount_percent, 23);
    }

    #[test]
    fn only_ozon_picks_deals_but_market_counts_every_seller() {
        let mut offers = market_offers();
        offers.push(phone(55_000));
        offers.push(Offer {
            sold_by_ozon: true,
            ..phone(58_000)
        });
        let ozon_only = Watch {
            shop: ShopConditions {
                only_ozon: true,
                ..ShopConditions::default()
            },
            ..iphone_watch()
        };

        let assessment = assess(&ozon_only, offers, Local::now());

        assert_eq!(assessment.comparable, 8, "в рынке и сторонний продавец за 55 000 ₽");
        assert_eq!(assessment.market, Some(MarketPrice::Whole(71_500)));
        assert_eq!(
            assessment.suitable.iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [58_000]
        );
    }

    #[test]
    fn suspiciously_cheap_offers_are_hidden_unless_asked_for() {
        let mut offers = market_offers();
        offers.push(phone(20_000));
        offers.push(phone(58_000));

        let cautious = assess(&iphone_watch(), offers.clone(), Local::now());
        assert_eq!(
            cautious.suitable.iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [58_000]
        );
        assert_eq!(cautious.hidden_suspicious, 1);

        let curious = Watch {
            include_suspicious: true,
            ..iphone_watch()
        };
        let all = assess(&curious, offers, Local::now());
        assert_eq!(
            all.suitable.iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [20_000, 58_000]
        );
        assert!(all.suitable[0].suspicious);
        assert_eq!(all.hidden_suspicious, 0);
    }

    #[test]
    fn market_counts_whole_area_but_deals_only_within_radius() {
        let listing = |price: u64, km: f64| Offer {
            url: format!("https://www.avito.ru/{price}"),
            distance_km: Some(km),
            ..Offer::sample("Шуруповерт Makita DF333", price)
        };
        let offers = vec![
            listing(5_000, 1.0),
            listing(5_200, 20.0),
            listing(5_100, 12.0),
            listing(4_900, 8.0),
            listing(5_300, 3.0),
            listing(3_000, 2.0),
            listing(3_100, 9.0),
        ];
        let nearby = Watch {
            name: "Шуруповёрт".into(),
            query: "шуруповерт makita".into(),
            source: SourceKind::Avito,
            category: Category::Tools,
            radius_km: Some(5),
            ..iphone_watch()
        };

        let assessment = assess(&nearby, offers, Local::now());

        assert_eq!(assessment.market, Some(MarketPrice::Whole(5_000)));
        assert_eq!(
            assessment.suitable.iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [3_000]
        );
    }

    #[test]
    fn copies_of_one_listing_count_once() {
        let listing = |price: u64, place: &str, id: u32| Offer {
            url: format!("https://www.avito.ru/{id}"),
            place: Some(place.into()),
            distance_km: Some(2.4),
            ..Offer::sample("Шуруповерт Makita 18V", price)
        };
        let mut offers: Vec<Offer> = (0..4).map(|id| listing(3_510, "Валовая ул. Павелецкая", id)).collect();
        offers.extend([
            listing(6_000, "Люблино", 10),
            listing(6_500, "Строгино", 11),
            listing(7_000, "Ясенево", 12),
            listing(7_500, "Битца", 13),
        ]);
        let nearby = Watch {
            query: "шуруповерт makita".into(),
            category: Category::Tools,
            ..iphone_watch()
        };

        let assessment = assess(&nearby, offers, Local::now());

        assert_eq!(assessment.comparable, 5);
        assert_eq!(assessment.market, Some(MarketPrice::Whole(6_500)));
        assert_eq!(assessment.suitable.len(), 1);
    }

    #[test]
    fn too_few_similar_offers_give_no_deals() {
        let offers = [70_000, 50_000, 72_000].map(phone).to_vec();
        let assessment = assess(&iphone_watch(), offers, Local::now());
        assert!(assessment.market.is_none());
        assert!(assessment.suitable.is_empty());
    }

    #[test]
    fn consumables_are_compared_per_kilogram_not_per_pack() {
        let pack = |title: &str, price: u64| Offer {
            url: format!("https://example.com/{price}"),
            ..Offer::sample(title, price)
        };
        let offers = vec![
            pack("Стиральный порошок Persil 450 г", 150),
            pack("Стиральный порошок Persil 3 кг", 900),
            pack("Стиральный порошок Persil 3 кг", 960),
            pack("Стиральный порошок Persil 4,5 кг", 1_440),
            pack("Стиральный порошок Persil 6 кг", 1_920),
            pack("Стиральный порошок Persil 9 кг", 2_070),
            pack("Гель для стирки Persil 1 л", 250),
            pack("Стиральный порошок Persil", 99),
        ];
        let powder = Watch {
            query: "persil".into(),
            category: Category::Consumables,
            goal: Goal::BelowMarket {
                min_discount_percent: 20,
            },
            ..iphone_watch()
        };

        let assessment = assess(&powder, offers, Local::now());

        assert_eq!(
            assessment.comparable, 6,
            "гель в литрах и пачка без веса с килограммами не сравниваются"
        );
        assert_eq!(assessment.market.unwrap().to_string(), "320\u{a0}₽/кг");
        let [deal] = assessment.suitable.as_slice() else {
            panic!("выгодной должна быть одна пачка 9 кг, а не самая дешёвая на 450 г");
        };
        assert_eq!(deal.offer.price, 2_070);
        assert_eq!(deal.market_price, 2_880);
        assert_eq!(deal.discount_percent, 28);
        assert_eq!(deal.per_unit.unwrap().price.to_string(), "230\u{a0}₽/кг");
    }

    #[test]
    fn consumables_without_pack_sizes_have_no_market() {
        let offers = (1..=6).map(|i| Offer::sample("Губка для посуды", 100 + i)).collect();
        let sponges = Watch {
            query: "губка".into(),
            category: Category::Consumables,
            ..iphone_watch()
        };
        let assessment = assess(&sponges, offers, Local::now());
        assert_eq!(assessment.comparable, 0);
        assert!(assessment.market.is_none());
    }
}
