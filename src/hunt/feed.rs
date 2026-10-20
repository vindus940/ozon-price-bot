use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::hunt::deal::Deal;

pub const FILE: &str = "deals.json";
const KEPT: usize = 500;

/// Лента находок для вкладки «Находки»: свежие первыми, одно предложение одной слежки — одна запись.
/// Подешевевшее предложение не дублируется, а поднимается наверх с новой ценой.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Feed {
    deals: VecDeque<Deal>,
}

impl Feed {
    pub fn add(&mut self, news: &[Deal]) {
        for deal in news.iter().rev() {
            self.deals
                .retain(|old| !(old.watch_id == deal.watch_id && old.offer.url == deal.offer.url));
            self.deals.push_front(deal.clone());
        }
        self.deals.truncate(KEPT);
    }

    pub fn deals(&self) -> Vec<Deal> {
        self.deals.iter().cloned().collect()
    }

    pub fn count(&self) -> usize {
        self.deals.len()
    }

    pub fn clear(&mut self) {
        self.deals.clear();
    }

    pub fn remove(&mut self, watch_id: u64, url: &str) -> Option<Deal> {
        let index = self
            .deals
            .iter()
            .position(|deal| deal.watch_id == watch_id && deal.offer.url == url)?;
        self.deals.remove(index)
    }
}

#[cfg(test)]
mod tests {
    use chrono::Local;

    use super::*;
    use crate::offer::Offer;

    fn deal(watch_id: u64, url: &str, price: u64) -> Deal {
        Deal {
            watch_id,
            watch_name: "Айфон".into(),
            found_at: Local::now(),
            offer: Offer {
                url: url.into(),
                ..Offer::sample("iPhone 15", price)
            },
            market_price: 70_000,
            market_sample: 9,
            discount_percent: 20,
            resale_profit: None,
            suspicious: false,
            per_unit: None,
        }
    }

    fn prices(feed: &Feed) -> Vec<u64> {
        feed.deals().iter().map(|deal| deal.offer.price).collect()
    }

    #[test]
    fn removed_is_only_that_offer_of_that_watch() {
        let mut feed = Feed::default();
        feed.add(&[deal(1, "a", 50_000), deal(2, "a", 51_000), deal(1, "b", 52_000)]);

        assert_eq!(feed.remove(1, "a").map(|deal| deal.offer.price), Some(50_000));
        assert_eq!(prices(&feed), [51_000, 52_000]);
        assert!(feed.remove(1, "a").is_none());
    }

    #[test]
    fn fresh_news_go_on_top_in_their_own_order() {
        let mut feed = Feed::default();
        feed.add(&[deal(1, "a", 50_000)]);
        feed.add(&[deal(1, "b", 52_000), deal(1, "c", 53_000)]);
        assert_eq!(prices(&feed), [52_000, 53_000, 50_000]);
    }

    #[test]
    fn cheaper_again_offer_replaces_its_old_entry() {
        let mut feed = Feed::default();
        feed.add(&[deal(1, "a", 58_000), deal(1, "b", 60_000)]);
        feed.add(&[deal(1, "a", 55_000)]);
        assert_eq!(prices(&feed), [55_000, 60_000]);
    }

    #[test]
    fn saved_as_plain_list_so_older_files_still_load() {
        let mut feed = Feed::default();
        feed.add(&[deal(1, "a", 58_000)]);

        let saved = serde_json::to_value(&feed).unwrap();

        assert!(saved.is_array());
        assert_eq!(prices(&serde_json::from_value(saved).unwrap()), [58_000]);
    }

    #[test]
    fn same_offer_in_two_watches_is_two_entries() {
        let mut feed = Feed::default();
        feed.add(&[deal(1, "a", 58_000)]);
        feed.add(&[deal(2, "a", 58_000)]);
        assert_eq!(feed.count(), 2);
    }
}
