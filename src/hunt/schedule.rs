use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Local};

use crate::hunt::market::MarketPrice;
use crate::hunt::watch::Watch;

#[derive(Debug, Clone)]
pub struct RunStatus {
    pub at: DateTime<Local>,
    pub outcome: Outcome,
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Checked {
        scanned: usize,
        comparable: usize,
        market: Option<MarketPrice>,
        suitable: usize,
        news: usize,
        hidden_suspicious: usize,
    },
    Failed(String),
}

/// Когда какую слежку проверять: итоги последних проверок, очередь «Проверить сейчас»
/// и слежка, которая проверяется в эту минуту. Итоги живут только в памяти: после перезапуска
/// все слежки проверяются сразу.
#[derive(Debug, Default)]
pub struct Schedule {
    runs: HashMap<u64, RunStatus>,
    queue: VecDeque<u64>,
    checking: Option<u64>,
}

impl Schedule {
    pub fn status(&self, watch_id: u64) -> Option<RunStatus> {
        self.runs.get(&watch_id).cloned()
    }

    pub fn is_checking(&self, watch_id: u64) -> bool {
        self.checking == Some(watch_id)
    }

    pub fn is_queued(&self, watch_id: u64) -> bool {
        self.queue.contains(&watch_id)
    }

    pub fn check_soon(&mut self, watch_id: u64) {
        self.queue.push_back(watch_id);
    }

    /// Сначала то, что попросили проверить кнопкой, потом первая слежка, которой пора по расписанию.
    pub fn next(&mut self, watches: &[Watch], now: DateTime<Local>) -> Option<u64> {
        if let Some(watch_id) = self.queue.pop_front() {
            return Some(watch_id);
        }
        watches
            .iter()
            .find(|watch| watch.is_due(self.runs.get(&watch.id).map(|run| run.at), now))
            .map(|watch| watch.id)
    }

    pub fn start(&mut self, watch_id: u64) {
        self.checking = Some(watch_id);
    }

    pub fn finish(&mut self) {
        self.checking = None;
    }

    /// Запоминает итог проверки. `true` — слежка только что сломалась: до этого она работала или ещё
    /// не проверялась. О таком сбое стоит сообщить, о повторных — нет.
    pub fn record(&mut self, watch_id: u64, at: DateTime<Local>, outcome: Outcome) -> bool {
        let newly_failed = matches!(outcome, Outcome::Failed(_)) && !self.is_failing(watch_id);
        self.runs.insert(watch_id, RunStatus { at, outcome });
        newly_failed
    }

    /// Слежку изменили или удалили: её прошлые итоги больше ни о чём не говорят.
    pub fn forget(&mut self, watch_id: u64) {
        self.runs.remove(&watch_id);
    }

    /// Упавшие слежки проверяются сразу, а не через свой интервал: например, когда указали дом, которого им не хватало.
    pub fn retry_failed(&mut self) {
        self.runs.retain(|_, run| !matches!(run.outcome, Outcome::Failed(_)));
    }

    fn is_failing(&self, watch_id: u64) -> bool {
        self.runs
            .get(&watch_id)
            .is_some_and(|run| matches!(run.outcome, Outcome::Failed(_)))
    }
}

#[cfg(test)]
mod tests {
    use chrono::Duration;

    use super::*;

    fn watch(id: u64) -> Watch {
        Watch {
            id,
            every_minutes: 15,
            ..Watch::new_draft()
        }
    }

    fn checked() -> Outcome {
        Outcome::Checked {
            scanned: 80,
            comparable: 40,
            market: None,
            suitable: 0,
            news: 0,
            hidden_suspicious: 0,
        }
    }

    #[test]
    fn button_goes_first_then_whatever_is_due() {
        let now = Local::now();
        let watches = [watch(1), watch(2)];
        let mut schedule = Schedule::default();
        schedule.record(1, now, checked());
        schedule.check_soon(2);
        schedule.check_soon(1);

        assert_eq!(schedule.next(&watches, now), Some(2));
        assert_eq!(schedule.next(&watches, now), Some(1));
        assert_eq!(
            schedule.next(&watches, now),
            Some(2),
            "вторая ещё ни разу не проверялась"
        );
        schedule.record(2, now, checked());
        assert_eq!(schedule.next(&watches, now), None);
        assert_eq!(schedule.next(&watches, now + Duration::minutes(15)), Some(1));
    }

    #[test]
    fn failure_is_news_only_after_success_or_first_time() {
        let now = Local::now();
        let mut schedule = Schedule::default();
        let failed = || Outcome::Failed("капча".into());

        assert!(schedule.record(1, now, failed()));
        assert!(!schedule.record(1, now, failed()));
        assert!(!schedule.record(1, now, checked()));
        assert!(schedule.record(1, now, failed()));
    }

    #[test]
    fn retry_failed_makes_only_failed_watches_due_now() {
        let now = Local::now();
        let watches = [watch(1), watch(2)];
        let mut schedule = Schedule::default();
        schedule.record(1, now, checked());
        schedule.record(2, now, Outcome::Failed("нет дома".into()));

        schedule.retry_failed();

        assert_eq!(schedule.next(&watches, now), Some(2));
        assert!(schedule.status(1).is_some());
    }
}
