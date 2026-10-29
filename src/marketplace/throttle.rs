use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use reqwest::Url;
use tokio::sync::Mutex;
use tokio::time::{Instant, sleep};

use crate::marketplace::{OfferSource, Product, Scope, SourceError};
use crate::offer::Offer;

/// Площадка с паузой между поисками. Пауза общая для всех, кто ищет на площадке, — слежек, бота и самопроверки:
/// сайту всё равно, кто из них спрашивает, а частые запросы — первый повод показать проверку «я не робот».
pub struct Throttled {
    inner: Arc<dyn OfferSource>,
    gap: Duration,
    last_start: Mutex<Option<Instant>>,
}

impl Throttled {
    pub fn new(inner: Arc<dyn OfferSource>, gap: Duration) -> Self {
        Self {
            inner,
            gap,
            last_start: Mutex::new(None),
        }
    }

    /// Поиск начинается не раньше чем через `gap` после начала предыдущего; ждущие встают в очередь.
    async fn wait_turn(&self) {
        let mut last_start = self.last_start.lock().await;
        let delay = delay_before(*last_start, Instant::now(), self.gap);
        if !delay.is_zero() {
            sleep(delay).await;
        }
        *last_start = Some(Instant::now());
    }
}

#[async_trait]
impl OfferSource for Throttled {
    fn name(&self) -> &'static str {
        self.inner.name()
    }

    async fn search(&self, query: &str, scope: Scope) -> Result<Vec<Offer>, SourceError> {
        self.wait_turn().await;
        self.inner.search(query, scope).await
    }

    async fn search_link(&self, link: &Url, scope: Scope) -> Result<Vec<Offer>, SourceError> {
        self.wait_turn().await;
        self.inner.search_link(link, scope).await
    }

    async fn product(&self, link: &Url) -> Result<Product, SourceError> {
        self.wait_turn().await;
        self.inner.product(link).await
    }
}

fn delay_before(last_start: Option<Instant>, now: Instant, gap: Duration) -> Duration {
    last_start.map_or(Duration::ZERO, |last| (last + gap).saturating_duration_since(now))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marketplace::testing::FakeSource;

    #[test]
    fn waits_only_what_is_left_of_the_gap() {
        let now = Instant::now();
        let gap = Duration::from_secs(15);
        assert_eq!(delay_before(None, now, gap), Duration::ZERO, "первый поиск — сразу");
        assert_eq!(
            delay_before(Some(now), now + Duration::from_secs(5), gap),
            Duration::from_secs(10)
        );
        assert_eq!(
            delay_before(Some(now), now + Duration::from_secs(20), gap),
            Duration::ZERO
        );
    }

    #[tokio::test]
    async fn second_search_waits_for_the_gap_and_both_reach_the_site() {
        let fake = FakeSource::returning(Vec::new());
        let queries = fake.queries();
        let throttled = Throttled::new(Arc::new(fake), Duration::from_millis(300));

        let started = Instant::now();
        throttled.search("наушники", Scope::default()).await.unwrap();
        throttled.search("чайник", Scope::default()).await.unwrap();

        assert!(
            started.elapsed() >= Duration::from_millis(300),
            "второй поиск не подождал"
        );
        assert_eq!(*queries.lock().unwrap(), ["наушники", "чайник"]);
    }
}
