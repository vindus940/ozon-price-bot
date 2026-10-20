use crate::hunt::assess::Assessment;
use crate::hunt::watch::Watch;

/// Что видно в форме о пробной проверке.
#[derive(Debug, Clone)]
pub enum TrialStatus {
    /// Ждёт, пока закончится текущая проверка: площадку не дёргаем параллельно.
    Queued,
    Checking,
    Done(Assessment),
    Failed(String),
}

/// Пробная проверка слежки, которую ещё не сохранили. Одна на всё окно: новая заменяет прежнюю,
/// а итог прежней, пришедший уже после замены, отбрасывается.
#[derive(Debug, Default)]
pub struct Trials {
    current: Option<Trial>,
    requested: u64,
}

#[derive(Debug)]
struct Trial {
    number: u64,
    watch: Watch,
    status: TrialStatus,
}

impl Trials {
    pub fn request(&mut self, watch: Watch) {
        self.requested += 1;
        self.current = Some(Trial {
            number: self.requested,
            watch,
            status: TrialStatus::Queued,
        });
    }

    /// Пробная проверка, которая ждёт своей очереди: с этого момента она идёт.
    pub fn start(&mut self) -> Option<(u64, Watch)> {
        let trial = self.current.as_mut()?;
        if !matches!(trial.status, TrialStatus::Queued) {
            return None;
        }
        trial.status = TrialStatus::Checking;
        Some((trial.number, trial.watch.clone()))
    }

    pub fn finish(&mut self, number: u64, status: TrialStatus) {
        if let Some(trial) = self.current.as_mut().filter(|trial| trial.number == number) {
            trial.status = status;
        }
    }

    pub fn status(&self) -> Option<TrialStatus> {
        self.current.as_ref().map(|trial| trial.status.clone())
    }

    pub fn forget(&mut self) {
        self.current = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hunt::fixtures::iphone_watch;

    #[test]
    fn trial_starts_once_and_keeps_its_result() {
        let mut trials = Trials::default();
        trials.request(iphone_watch());

        let (number, watch) = trials.start().unwrap();
        assert_eq!(watch.query, "iphone 15 128");
        assert!(trials.start().is_none(), "идущая проверка не запускается второй раз");
        assert!(matches!(trials.status(), Some(TrialStatus::Checking)));

        trials.finish(number, TrialStatus::Failed("нет сети".into()));
        assert!(matches!(trials.status(), Some(TrialStatus::Failed(reason)) if reason == "нет сети"));
        assert!(trials.start().is_none());
    }

    #[test]
    fn result_of_replaced_trial_is_dropped() {
        let mut trials = Trials::default();
        trials.request(iphone_watch());
        let (old, _) = trials.start().unwrap();
        trials.request(iphone_watch());

        trials.finish(old, TrialStatus::Failed("старая".into()));

        assert!(matches!(trials.status(), Some(TrialStatus::Queued)));
    }

    #[test]
    fn forgotten_trial_neither_runs_nor_shows() {
        let mut trials = Trials::default();
        trials.request(iphone_watch());
        trials.forget();
        assert!(trials.start().is_none());
        assert!(trials.status().is_none());
    }
}
