use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::hunt::deal::Deal;

pub const FILE: &str = "notified.json";

/// Снова сообщить об уже известном предложении стоит, только если оно заметно подешевело.
const NOTEWORTHY_DROP_PERCENT: u64 = 3;

/// Что уже отправлено по каждой слежке: ссылка → цена на момент уведомления.
/// Без этой памяти каждые 15 минут приходили бы одни и те же находки.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Notified {
    by_watch: HashMap<u64, HashMap<String, u64>>,
    /// Находки, которые вы отметили «мимо»: их слежка не присылает больше никогда, даже подешевевшими.
    /// Правка слежки их не сбрасывает — подделка остаётся подделкой и при других условиях.
    #[serde(default)]
    dismissed: HashMap<u64, HashSet<String>>,
    /// Слежки, о сбое которых уже сообщили. В файле, а не в памяти программы: иначе каждый перезапуск
    /// присылал бы то же «не смогла проверить» снова.
    #[serde(default)]
    failing: HashSet<u64>,
    /// Слежки за товаром, о которых уже сообщили «цена дошла до цели», — с ценой того сообщения.
    #[serde(default)]
    at_target: HashMap<u64, u64>,
}

/// Что сообщить о цене товара после проверки.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PriceNews {
    /// Цена дошла до цели. `before` — цена прошлого такого сообщения, если товар с тех пор заметно подешевел ещё.
    Reached {
        before: Option<u64>,
    },
    /// Была у цели, а теперь дороже её или товара нет в продаже.
    Rose,
    Quiet,
}

impl Notified {
    /// Цена товара против цели: о достижении — один раз и снова, только если товар заметно подешевел ещё;
    /// о том, что цена ушла выше, — один раз. `price: None` — товара нет в продаже.
    pub fn take_price_news(&mut self, watch_id: u64, target: u64, price: Option<u64>) -> PriceNews {
        self.mark_started(watch_id);
        let reached = price.filter(|&price| price <= target);
        match (reached, self.at_target.get(&watch_id).copied()) {
            (Some(price), None) => {
                self.at_target.insert(watch_id, price);
                PriceNews::Reached { before: None }
            }
            (Some(price), Some(before)) if noteworthy_drop(price, before) => {
                self.at_target.insert(watch_id, price);
                PriceNews::Reached { before: Some(before) }
            }
            (None, Some(_)) => {
                self.at_target.remove(&watch_id);
                PriceNews::Rose
            }
            _ => PriceNews::Quiet,
        }
    }

    /// Цена последнего сообщения «дошла до цели» — чтобы вернуть её, если уведомление не ушло.
    pub fn price_at_target(&self, watch_id: u64) -> Option<u64> {
        self.at_target.get(&watch_id).copied()
    }

    /// Уведомление о цене не ушло — память как до проверки, и при следующей проверке оно уйдёт снова.
    pub fn restore_price_at_target(&mut self, watch_id: u64, price: Option<u64>) {
        match price {
            Some(price) => self.at_target.insert(watch_id, price),
            None => self.at_target.remove(&watch_id),
        };
    }

    /// Что из найденного стоит отправить: новое и заметно подешевевшее. Отобранное сразу запоминается,
    /// а слежка считается запущенной, даже если отправлять нечего. У подешевевшего — прошлая цена.
    pub fn take_news(&mut self, watch_id: u64, deals: &[Deal]) -> Vec<Deal> {
        let news: Vec<Deal> = deals
            .iter()
            .filter(|deal| self.is_news(watch_id, &deal.offer.url, deal.offer.price))
            .map(|deal| Deal {
                price_before: self.notified_price(watch_id, &deal.offer.url),
                ..deal.clone()
            })
            .collect();
        self.mark_started(watch_id);
        for deal in &news {
            self.remember(watch_id, &deal.offer.url, deal.offer.price);
        }
        news
    }

    /// Первая проверка слежки: всё найденное — не новости, а отправная точка.
    pub fn is_first_run(&self, watch_id: u64) -> bool {
        !self.by_watch.contains_key(&watch_id)
    }

    fn is_news(&self, watch_id: u64, url: &str, price: u64) -> bool {
        if self.dismissed.get(&watch_id).is_some_and(|urls| urls.contains(url)) {
            return false;
        }
        self.notified_price(watch_id, url)
            .is_none_or(|notified_price| noteworthy_drop(price, notified_price))
    }

    fn notified_price(&self, watch_id: u64, url: &str) -> Option<u64> {
        self.by_watch.get(&watch_id)?.get(url).copied()
    }

    fn remember(&mut self, watch_id: u64, url: &str, price: u64) {
        self.by_watch
            .entry(watch_id)
            .or_default()
            .insert(url.to_string(), price);
    }

    fn mark_started(&mut self, watch_id: u64) {
        self.by_watch.entry(watch_id).or_default();
    }

    /// Правка начинает слежку заново: и её сбой, если повторится, — уже новость.
    pub fn forget(&mut self, watch_id: u64) {
        self.by_watch.remove(&watch_id);
        self.failing.remove(&watch_id);
        self.at_target.remove(&watch_id);
    }

    pub fn failure_reported(&self, watch_id: u64) -> bool {
        self.failing.contains(&watch_id)
    }

    pub fn report_failure(&mut self, watch_id: u64) {
        self.failing.insert(watch_id);
    }

    pub fn report_recovery(&mut self, watch_id: u64) {
        self.failing.remove(&watch_id);
    }

    /// Слежку удалили: забыть о ней всё, и отметки «мимо» тоже.
    pub fn forget_watch(&mut self, watch_id: u64) {
        self.forget(watch_id);
        self.dismissed.remove(&watch_id);
    }

    pub fn dismiss(&mut self, watch_id: u64, url: &str) {
        self.dismissed.entry(watch_id).or_default().insert(url.to_string());
    }

    /// Уведомление не ушло — предложение снова считается новым и будет отправлено при следующей проверке.
    pub fn forget_offer(&mut self, watch_id: u64, url: &str) {
        if let Some(seen) = self.by_watch.get_mut(&watch_id) {
            seen.remove(url);
        }
    }
}

fn noteworthy_drop(price: u64, before: u64) -> bool {
    price * 100 <= before * (100 - NOTEWORTHY_DROP_PERCENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_price_is_reported_when_reached_again_only_much_cheaper_and_once_when_it_rises() {
        let mut notified = Notified::default();
        let mut check = |price| notified.take_price_news(9, 3_500, price);

        assert_eq!(check(Some(3_870)), PriceNews::Quiet, "ещё дороже цели");
        assert_eq!(check(Some(3_500)), PriceNews::Reached { before: None });
        assert_eq!(check(Some(3_450)), PriceNews::Quiet, "подешевело меньше чем на 3%");
        assert_eq!(check(Some(3_300)), PriceNews::Reached { before: Some(3_500) });
        assert_eq!(check(Some(3_600)), PriceNews::Rose);
        assert_eq!(check(Some(3_700)), PriceNews::Quiet, "о подорожании — один раз");
        assert_eq!(check(Some(3_400)), PriceNews::Reached { before: None });
        assert_eq!(check(None), PriceNews::Rose, "пропал из продажи");
    }

    #[test]
    fn target_price_memory_survives_restart_restores_after_failed_alert_and_resets_on_edit() {
        let mut notified = Notified::default();
        notified.take_price_news(9, 3_500, Some(3_400));
        let mut saved: Notified = serde_json::from_str(&serde_json::to_string(&notified).unwrap()).unwrap();
        assert_eq!(saved.take_price_news(9, 3_500, Some(3_400)), PriceNews::Quiet);

        let before = saved.price_at_target(9);
        assert_eq!(saved.take_price_news(9, 3_500, None), PriceNews::Rose);
        saved.restore_price_at_target(9, before);
        assert_eq!(
            saved.take_price_news(9, 3_500, None),
            PriceNews::Rose,
            "не ушло — уйдёт снова"
        );

        saved.take_price_news(9, 3_500, Some(3_400));
        saved.forget(9);
        assert_eq!(
            saved.take_price_news(9, 3_500, Some(3_400)),
            PriceNews::Reached { before: None }
        );
    }

    #[test]
    fn new_offer_and_noticeable_drop_are_news() {
        let mut notified = Notified::default();
        assert!(notified.is_first_run(7));
        assert!(notified.is_news(7, "a", 10_000));

        notified.remember(7, "a", 10_000);
        assert!(!notified.is_first_run(7));
        assert!(!notified.is_news(7, "a", 10_000));
        assert!(!notified.is_news(7, "a", 9_800));
        assert!(notified.is_news(7, "a", 9_700));
        assert!(notified.is_news(7, "b", 10_000));
    }

    fn deal(price: u64) -> Deal {
        Deal {
            watch_id: 7,
            watch_name: "Айфон".into(),
            found_at: chrono::Local::now(),
            offer: crate::offer::Offer {
                url: "https://example.com/a".into(),
                ..crate::offer::Offer::sample("iPhone 15", price)
            },
            market_price: 70_000,
            market_sample: 9,
            discount_percent: 20,
            resale_profit: None,
            suspicious: false,
            per_unit: None,
            price_before: None,
        }
    }

    #[test]
    fn offer_that_came_again_cheaper_carries_its_notified_price() {
        let mut notified = Notified::default();
        let before = |news: Vec<Deal>| news.iter().map(|deal| deal.price_before).collect::<Vec<_>>();

        assert_eq!(before(notified.take_news(7, &[deal(10_000)])), [None]);
        assert_eq!(before(notified.take_news(7, &[deal(9_800)])), []);
        assert_eq!(before(notified.take_news(7, &[deal(9_000)])), [Some(10_000)]);
        assert_eq!(
            before(notified.take_news(7, &[deal(8_500)])),
            [Some(9_000)],
            "сравнивается с последним уведомлением"
        );
    }

    #[test]
    fn watches_are_remembered_separately_and_can_be_forgotten() {
        let mut notified = Notified::default();
        notified.remember(1, "a", 100);
        notified.mark_started(2);
        assert!(notified.is_news(2, "a", 100));
        assert!(!notified.is_first_run(2));

        notified.forget(1);
        assert!(notified.is_first_run(1));
    }

    #[test]
    fn forgotten_offer_is_news_again_but_watch_keeps_its_start() {
        let mut notified = Notified::default();
        notified.remember(3, "a", 100);
        notified.forget_offer(3, "a");
        assert!(notified.is_news(3, "a", 100));
        assert!(!notified.is_first_run(3));
    }

    #[test]
    fn dismissed_offer_never_comes_back_until_the_watch_is_deleted() {
        let mut notified = Notified::default();
        notified.remember(4, "fake", 1_000);
        notified.dismiss(4, "fake");
        assert!(!notified.is_news(4, "fake", 500), "даже подешевев вдвое");
        assert!(notified.is_news(5, "fake", 1_000), "у другой слежки своё мнение");

        notified.forget(4);
        assert!(!notified.is_news(4, "fake", 1_000), "правка слежки отметку не снимает");

        notified.forget_watch(4);
        assert!(notified.is_news(4, "fake", 1_000));
    }

    #[test]
    fn memory_saved_before_dismissals_is_read() {
        let old: Notified = serde_json::from_str(r#"{"by_watch":{"1":{"https://example.com/a":100}}}"#).unwrap();
        assert!(!old.is_news(1, "https://example.com/a", 100));
        assert!(!old.failure_reported(1));
    }

    #[test]
    fn reported_failure_is_saved_and_cleared_by_recovery_or_edit() {
        let mut notified = Notified::default();
        notified.report_failure(1);
        notified.report_failure(2);

        let saved: Notified = serde_json::from_str(&serde_json::to_string(&notified).unwrap()).unwrap();
        assert!(saved.failure_reported(1), "отметка переживает перезапуск");

        notified.report_recovery(1);
        notified.forget(2);
        assert!(!notified.failure_reported(1) && !notified.failure_reported(2));
    }
}
