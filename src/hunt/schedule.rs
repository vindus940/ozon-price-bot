use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Local};

use crate::hunt::market::MarketPrice;
use crate::hunt::watch::Watch;
use crate::marketplace::SourceKind;

/// Пока на площадке открыта проверка «я не робот», повторные попытки только злят её: после первой такой проверки
/// слежки площадки ждут 15 минут, после второй подряд — час, после третьей — пока вы сами не нажмёте «Проверить сейчас».
const CAPTCHA_PAUSES_MINUTES: [i64; 2] = [15, 60];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pause {
    Until(DateTime<Local>),
    UntilManualCheck,
}

#[derive(Debug, Clone, Copy)]
struct Captchas {
    in_a_row: usize,
    pause: Pause,
}

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

/// Когда какую слежку проверять: итоги последних проверок, очередь «Проверить сейчас», слежка, которая
/// проверяется в эту минуту, и паузы площадок после проверок «я не робот». Всё это живёт только в памяти:
/// после перезапуска все слежки проверяются сразу.
#[derive(Debug, Default)]
pub struct Schedule {
    runs: HashMap<u64, RunStatus>,
    queue: VecDeque<u64>,
    checking: Option<u64>,
    captchas: HashMap<SourceKind, Captchas>,
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

    /// Сначала то, что попросили проверить кнопкой (кнопка обходит и паузу: вы у компьютера и видите окно браузера),
    /// потом первая слежка, которой пора по расписанию и чья площадка не на паузе.
    pub fn next(&mut self, watches: &[Watch], now: DateTime<Local>) -> Option<u64> {
        if let Some(watch_id) = self.queue.pop_front() {
            return Some(watch_id);
        }
        watches
            .iter()
            .find(|watch| {
                watch.is_due(self.runs.get(&watch.id).map(|run| run.at), now) && self.pause(watch.source, now).is_none()
            })
            .map(|watch| watch.id)
    }

    pub fn start(&mut self, watch_id: u64) {
        self.checking = Some(watch_id);
    }

    pub fn finish(&mut self) {
        self.checking = None;
    }

    pub fn record(&mut self, watch_id: u64, at: DateTime<Local>, outcome: Outcome) {
        self.runs.insert(watch_id, RunStatus { at, outcome });
    }

    /// Площадка показала проверку «я не робот» — на сколько встают её слежки.
    pub fn captcha(&mut self, source: SourceKind, now: DateTime<Local>) -> Pause {
        let in_a_row = self.captchas.get(&source).map_or(0, |captchas| captchas.in_a_row) + 1;
        let pause = CAPTCHA_PAUSES_MINUTES
            .get(in_a_row - 1)
            .map_or(Pause::UntilManualCheck, |&minutes| {
                Pause::Until(now + Duration::minutes(minutes))
            });
        self.captchas.insert(source, Captchas { in_a_row, pause });
        pause
    }

    /// Площадка отдала выдачу — счёт проверок «я не робот» подряд начинается заново.
    pub fn source_answered(&mut self, source: SourceKind) {
        self.captchas.remove(&source);
    }

    pub fn pause(&self, source: SourceKind, now: DateTime<Local>) -> Option<Pause> {
        match self.captchas.get(&source)?.pause {
            Pause::Until(until) if until <= now => None,
            pause => Some(pause),
        }
    }

    /// Слежку изменили или удалили: её прошлые итоги больше ни о чём не говорят.
    pub fn forget(&mut self, watch_id: u64) {
        self.runs.remove(&watch_id);
    }

    /// Упавшие слежки проверяются сразу, а не через свой интервал: например, когда указали дом, которого им не хватало.
    pub fn retry_failed(&mut self) {
        self.runs.retain(|_, run| !matches!(run.outcome, Outcome::Failed(_)));
    }
}

#[cfg(test)]
mod tests {
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
    fn captchas_in_a_row_pause_the_source_longer_then_wait_for_the_button() {
        let now = Local::now();
        let avito = [Watch {
            source: SourceKind::Avito,
            ..watch(2)
        }];
        let ozon = [watch(1)];
        let mut schedule = Schedule::default();

        assert_eq!(
            schedule.captcha(SourceKind::Avito, now),
            Pause::Until(now + Duration::minutes(15))
        );
        assert_eq!(
            schedule.next(&avito, now),
            None,
            "пора по расписанию, но площадка на паузе"
        );
        assert_eq!(schedule.next(&ozon, now), Some(1), "другие площадки не ждут");
        assert_eq!(schedule.next(&avito, now + Duration::minutes(15)), Some(2));

        assert_eq!(
            schedule.captcha(SourceKind::Avito, now),
            Pause::Until(now + Duration::minutes(60)),
            "вторая подряд — час"
        );
        assert_eq!(schedule.captcha(SourceKind::Avito, now), Pause::UntilManualCheck);
        assert_eq!(
            schedule.next(&avito, now + Duration::days(1)),
            None,
            "после третьей — только кнопкой"
        );
        schedule.check_soon(2);
        assert_eq!(
            schedule.next(&avito, now + Duration::days(1)),
            Some(2),
            "кнопка обходит паузу"
        );

        schedule.source_answered(SourceKind::Avito);
        assert_eq!(schedule.pause(SourceKind::Avito, now), None);
        assert_eq!(
            schedule.captcha(SourceKind::Avito, now),
            Pause::Until(now + Duration::minutes(15)),
            "после удачной проверки счёт начинается заново"
        );
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
