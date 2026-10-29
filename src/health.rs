use std::mem::{Discriminant, discriminant};
use std::sync::Arc;
use std::time::Duration;

use crate::marketplace::{OfferSource, Scope, SourceError};
use crate::offer::Offer;

const MIN_OFFERS: usize = 10;
const SAMPLE_SIZE: usize = 3;

pub struct SelfCheckSettings {
    pub every: Duration,
    pub query: String,
}

/// Контрольный поиск: проверяет, что площадка по-прежнему разбирается, а не молча отдаёт пустоту.
pub struct ParserCheck {
    source: Arc<dyn OfferSource>,
    settings: SelfCheckSettings,
}

#[derive(Debug)]
pub enum Health {
    Working { offers: usize },
    Broken { problems: Vec<Problem>, sample: Vec<Offer> },
    Unavailable(SourceError),
}

#[derive(Debug, PartialEq)]
pub enum Problem {
    TooFewOffers { found: usize, expected: usize },
    MissingNames { count: usize },
    NoRatings,
    NoReviews,
}

impl ParserCheck {
    pub fn new(source: Arc<dyn OfferSource>, settings: SelfCheckSettings) -> Self {
        Self { source, settings }
    }

    pub fn every(&self) -> Duration {
        self.settings.every
    }

    pub fn query(&self) -> &str {
        &self.settings.query
    }

    pub fn source_name(&self) -> &'static str {
        self.source.name()
    }

    pub async fn run(&self) -> Health {
        match self.source.search(&self.settings.query, Scope::default()).await {
            Ok(offers) => diagnose(offers),
            Err(error) => Health::Unavailable(error),
        }
    }
}

fn diagnose(offers: Vec<Offer>) -> Health {
    let mut problems = Vec::new();
    if offers.len() < MIN_OFFERS {
        problems.push(Problem::TooFewOffers {
            found: offers.len(),
            expected: MIN_OFFERS,
        });
    }
    let unnamed = offers.iter().filter(|offer| offer.name.is_empty()).count();
    if unnamed > 0 {
        problems.push(Problem::MissingNames { count: unnamed });
    }
    if !offers.is_empty() && offers.iter().all(|offer| offer.rating.is_none()) {
        problems.push(Problem::NoRatings);
    }
    if !offers.is_empty() && offers.iter().all(|offer| offer.reviews.is_none()) {
        problems.push(Problem::NoReviews);
    }

    if problems.is_empty() {
        Health::Working { offers: offers.len() }
    } else {
        Health::Broken {
            problems,
            sample: offers.into_iter().take(SAMPLE_SIZE).collect(),
        }
    }
}

/// Помнит прошлый результат, чтобы сообщать только о переменах, а не повторять одно и то же при каждой проверке.
/// До первой проверки считает, что всё работает: об исправной площадке при старте не пишет.
pub struct HealthMonitor {
    last: Discriminant<Health>,
}

impl Default for HealthMonitor {
    fn default() -> Self {
        Self {
            last: discriminant(&Health::Working { offers: 0 }),
        }
    }
}

impl HealthMonitor {
    pub fn changed(&mut self, health: &Health) -> bool {
        let current = discriminant(health);
        let changed = current != self.last;
        self.last = current;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marketplace::testing::FakeSource;

    fn rated(name: &str) -> Offer {
        Offer {
            rating: Some(4.8),
            reviews: Some(120),
            ..Offer::sample(name, 1000)
        }
    }

    fn healthy_offers() -> Vec<Offer> {
        (0..MIN_OFFERS).map(|i| rated(&format!("Наушники {i}"))).collect()
    }

    fn problems(health: Health) -> Vec<Problem> {
        match health {
            Health::Broken { problems, .. } => problems,
            other => panic!("ожидалась поломка, а пришло {other:?}"),
        }
    }

    #[test]
    fn enough_named_and_rated_offers_mean_working() {
        assert!(matches!(
            diagnose(healthy_offers()),
            Health::Working { offers: MIN_OFFERS }
        ));
    }

    #[test]
    fn empty_result_is_broken_and_has_empty_sample() {
        let health = diagnose(Vec::new());
        assert!(matches!(&health, Health::Broken { sample, .. } if sample.is_empty()));
        assert_eq!(
            problems(health),
            [Problem::TooFewOffers {
                found: 0,
                expected: MIN_OFFERS
            }]
        );
    }

    #[test]
    fn lost_names_ratings_and_reviews_are_reported_together() {
        let offers = (0..MIN_OFFERS).map(|_| Offer::sample("", 1000)).collect();
        assert_eq!(
            problems(diagnose(offers)),
            [
                Problem::MissingNames { count: MIN_OFFERS },
                Problem::NoRatings,
                Problem::NoReviews
            ]
        );
    }

    #[test]
    fn lost_reviews_alone_are_a_problem() {
        let offers = healthy_offers()
            .into_iter()
            .map(|offer| Offer { reviews: None, ..offer })
            .collect();
        assert_eq!(problems(diagnose(offers)), [Problem::NoReviews]);
    }

    #[test]
    fn broken_result_keeps_short_sample() {
        let mut offers = healthy_offers();
        offers[0].name.clear();
        assert!(matches!(diagnose(offers), Health::Broken { sample, .. } if sample.len() == SAMPLE_SIZE));
    }

    #[test]
    fn monitor_reports_only_changes() {
        let mut monitor = HealthMonitor::default();
        let broken = || Health::Broken {
            problems: vec![Problem::NoRatings],
            sample: Vec::new(),
        };

        assert!(!monitor.changed(&Health::Working { offers: 80 }));
        assert!(monitor.changed(&broken()));
        assert!(!monitor.changed(&broken()));
        assert!(monitor.changed(&Health::Unavailable(SourceError::Captcha)));
        assert!(monitor.changed(&Health::Working { offers: 80 }));
    }

    #[tokio::test]
    async fn check_searches_control_query_and_reports_unavailable_source() {
        let source = FakeSource::asking_captcha();
        let queries = source.queries();
        let check = ParserCheck::new(
            Arc::new(source),
            SelfCheckSettings {
                every: Duration::from_hours(6),
                query: "наушники".into(),
            },
        );

        assert!(matches!(check.run().await, Health::Unavailable(SourceError::Captcha)));
        assert_eq!(*queries.lock().unwrap(), ["наушники"]);
    }
}
