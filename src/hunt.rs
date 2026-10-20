mod assess;
mod book;
mod category;
mod deal;
mod feed;
#[cfg(test)]
mod fixtures;
mod journal;
mod market;
mod memory;
mod schedule;
mod trial;
mod watch;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use chrono::Local;
use serde::Serialize;
use tokio::sync::Notify;

use assess::assess;
pub use assess::{Assessment, Sifted};
use book::WatchBook;
pub use category::Category;
pub use deal::{Deal, Miss};
use feed::Feed;
use journal::Journal;
pub use journal::JournalEntry;
use market::MIN_SAMPLE;
/// По имени нужны только тестам текстов: остальной код берёт их из находки и итога проверки.
#[cfg(test)]
pub use market::{MarketPrice, PerUnit};
use memory::Notified;
use schedule::Schedule;
pub use schedule::{Outcome, Pause, RunStatus};
pub use trial::TrialStatus;
use trial::Trials;
pub use watch::{Goal, Watch};

use crate::geo::GeoPoint;
use crate::marketplace::{OfferSource, SourceError, SourceKind};
use crate::offer::Offer;
use crate::storage::Storage;

const SCHEDULE_TICK: Duration = Duration::from_secs(30);

pub enum Alert<'a> {
    /// Первая проверка слежки: что подходит прямо сейчас. Дальше — только новое.
    Started {
        watch: &'a Watch,
        deals: &'a [Deal],
    },
    NewDeals {
        watch: &'a Watch,
        deals: &'a [Deal],
    },
    /// Слежка перестала работать. Приходит один раз, а не при каждой неудачной проверке.
    Failed {
        watch: &'a Watch,
        reason: &'a str,
    },
    Test,
}

/// Куда отправлять находки. Охотник не знает, что это Telegram.
#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, alert: Alert<'_>) -> anyhow::Result<()>;
}

/// Всё, что меняется во время работы, — под одним замком: окно читает, проверки пишут.
struct State {
    book: WatchBook,
    schedule: Schedule,
    feed: Feed,
    notified: Notified,
    journal: Journal,
    trials: Trials,
    home: Option<GeoPoint>,
}

impl State {
    fn save_watches(&mut self, storage: &Storage) {
        save(storage, &mut self.journal, book::FILE, &self.book);
    }

    fn save_deals(&mut self, storage: &Storage) {
        save(storage, &mut self.journal, feed::FILE, &self.feed);
    }

    fn save_notified(&mut self, storage: &Storage) {
        save(storage, &mut self.journal, memory::FILE, &self.notified);
    }
}

/// Не сохранилось — пишем в журнал и работаем дальше: в памяти всё есть, файл перепишется при следующем изменении.
fn save(storage: &Storage, journal: &mut Journal, file: &str, value: &impl Serialize) {
    if let Err(error) = storage.save(file, value) {
        journal.write(format!("Не удалось сохранить {file}: {error:#}"));
    }
}

/// Охотник связывает части: берёт слежку из списка, когда подходит её время, ищет на площадке, оценивает найденное,
/// записывает итог и сообщает о новом. Сами правила — в частях: что выгодно — `assess`, когда проверять — `schedule`,
/// что уже отправлено — `memory`.
pub struct Hunter {
    sources: HashMap<SourceKind, Arc<dyn OfferSource>>,
    notifier: Option<Arc<dyn Notifier>>,
    storage: Storage,
    state: Mutex<State>,
    wake: Notify,
}

impl Hunter {
    pub fn new(
        sources: HashMap<SourceKind, Arc<dyn OfferSource>>,
        notifier: Option<Arc<dyn Notifier>>,
        storage: Storage,
        home: Option<GeoPoint>,
    ) -> Self {
        let state = State {
            book: storage.load(book::FILE),
            schedule: Schedule::default(),
            feed: storage.load(feed::FILE),
            notified: storage.load(memory::FILE),
            journal: Journal::default(),
            trials: Trials::default(),
            home,
        };
        Self {
            sources,
            notifier,
            storage,
            state: Mutex::new(state),
            wake: Notify::new(),
        }
    }

    pub fn watches(&self) -> Vec<Watch> {
        self.state().book.all().to_vec()
    }

    pub fn run_status(&self, watch_id: u64) -> Option<RunStatus> {
        self.state().schedule.status(watch_id)
    }

    /// Находки, свежие первыми.
    pub fn deals(&self) -> Vec<Deal> {
        self.state().feed.deals()
    }

    pub fn deals_count(&self) -> usize {
        self.state().feed.count()
    }

    pub fn journal(&self) -> Vec<JournalEntry> {
        self.state().journal.entries()
    }

    pub fn home(&self) -> Option<GeoPoint> {
        self.state().home
    }

    pub fn notifications_enabled(&self) -> bool {
        self.notifier.is_some()
    }

    pub fn is_connected(&self, source: SourceKind) -> bool {
        self.sources.contains_key(&source)
    }

    pub fn is_checking(&self, watch_id: u64) -> bool {
        self.state().schedule.is_checking(watch_id)
    }

    pub fn is_queued(&self, watch_id: u64) -> bool {
        self.state().schedule.is_queued(watch_id)
    }

    /// Ждут ли слежки площадки после проверки «я не робот».
    pub fn source_pause(&self, source: SourceKind) -> Option<Pause> {
        self.state().schedule.pause(source, Local::now())
    }

    /// Новая слежка (id = 0) или правка существующей. Правка начинает слежку заново:
    /// старая память об отправленном относится к другим условиям.
    pub fn save_watch(&self, watch: Watch) -> Result<u64, Vec<String>> {
        let mut state = self.state();
        let problems = watch.problems(state.home.is_some());
        if !problems.is_empty() {
            return Err(problems);
        }
        let name = watch.name.clone();
        let watch_id = state.book.save(watch).map_err(|problem| vec![problem])?;
        state.notified.forget(watch_id);
        state.schedule.forget(watch_id);
        state.journal.write(format!("Слежка «{name}» сохранена"));
        state.save_watches(&self.storage);
        state.save_notified(&self.storage);
        drop(state);
        self.wake.notify_one();
        Ok(watch_id)
    }

    pub fn delete_watch(&self, watch_id: u64) {
        let mut state = self.state();
        let Some(watch) = state.book.remove(watch_id) else {
            return;
        };
        state.notified.forget_watch(watch_id);
        state.schedule.forget(watch_id);
        state.journal.write(format!("Слежка «{}» удалена", watch.name));
        state.save_watches(&self.storage);
        state.save_notified(&self.storage);
    }

    pub fn set_enabled(&self, watch_id: u64, enabled: bool) {
        let mut state = self.state();
        if state.book.set_enabled(watch_id, enabled) {
            state.save_watches(&self.storage);
        }
    }

    pub fn run_now(&self, watch_id: u64) {
        self.state().schedule.check_soon(watch_id);
        self.wake.notify_one();
    }

    /// Проверить слежку, не сохраняя: встаёт первой в общую очередь, ничего не записывает в находки
    /// и в память об отправленном и ничего не отправляет. Итог — в `trial`.
    pub fn try_watch(&self, watch: Watch) -> Result<(), Vec<String>> {
        let mut state = self.state();
        let problems = watch.problems(state.home.is_some());
        if !problems.is_empty() {
            return Err(problems);
        }
        state.trials.request(watch);
        drop(state);
        self.wake.notify_one();
        Ok(())
    }

    pub fn trial(&self) -> Option<TrialStatus> {
        self.state().trials.status()
    }

    pub fn forget_trial(&self) {
        self.state().trials.forget();
    }

    /// Слежки, которые ждали дома и упали без него, проверяются сразу, а не через свой интервал.
    pub fn set_home(&self, home: Option<GeoPoint>) {
        let mut state = self.state();
        state.home = home;
        state.schedule.retry_failed();
        drop(state);
        self.wake.notify_one();
    }

    /// «Мимо»: находка уходит из ленты, и эта слежка больше её не пришлёт, даже подешевевшую.
    /// Возвращает слова из её названия, которые стоит предложить в стоп-слова, — пусто для «уже продано».
    pub fn dismiss(&self, watch_id: u64, url: &str, miss: Miss) -> Vec<String> {
        let mut state = self.state();
        let Some(deal) = state.feed.remove(watch_id, url) else {
            return Vec::new();
        };
        state.notified.dismiss(watch_id, url);
        state.journal.write(format!(
            "«{}»: «{}» — мимо, {}",
            deal.watch_name,
            deal.offer.name,
            miss.title()
        ));
        state.save_deals(&self.storage);
        state.save_notified(&self.storage);
        if !miss.suggests_stop_words() {
            return Vec::new();
        }
        let kept = state.feed.deals();
        let kept: Vec<&str> = kept
            .iter()
            .filter(|other| other.watch_id == watch_id)
            .map(|other| other.offer.name.as_str())
            .collect();
        state
            .book
            .get(watch_id)
            .map(|watch| watch.stop_word_candidates(&deal.offer.name, &kept))
            .unwrap_or_default()
    }

    /// Стоп-слово только сужает слежку, поэтому память об отправленном остаётся: в отличие от правки
    /// в форме, слежка не начинается заново и не присылает то же самое второй раз.
    pub fn add_stop_word(&self, watch_id: u64, word: &str) {
        let mut state = self.state();
        if !state.book.add_stop_word(watch_id, word) {
            return;
        }
        let name = state
            .book
            .get(watch_id)
            .map(|watch| watch.name.clone())
            .unwrap_or_default();
        state.journal.write(format!("Слежка «{name}»: стоп-слово «{word}»"));
        state.save_watches(&self.storage);
    }

    pub fn clear_deals(&self) {
        let mut state = self.state();
        state.feed.clear();
        state.save_deals(&self.storage);
    }

    pub async fn send_test(&self) -> anyhow::Result<()> {
        let notifier = self
            .notifier
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("Telegram не настроен"))?;
        notifier.notify(Alert::Test).await
    }

    /// Проверяет слежки по расписанию и по кнопке «Проверить сейчас» — строго по одной,
    /// чтобы не дёргать площадку параллельно. Пробная проверка из формы — первой: её ждут в окне.
    pub async fn run(self: Arc<Self>) {
        loop {
            self.check_pending().await;
            tokio::select! {
                () = tokio::time::sleep(SCHEDULE_TICK) => {}
                () = self.wake.notified() => {}
            }
        }
    }

    async fn check_pending(&self) {
        loop {
            let trial = self.state().trials.start();
            if let Some((number, watch)) = trial {
                self.try_out(number, &watch).await;
            } else if let Some(watch_id) = self.next_to_check() {
                self.check(watch_id).await;
            } else {
                return;
            }
        }
    }

    fn next_to_check(&self) -> Option<u64> {
        let mut guard = self.state();
        let state = &mut *guard;
        state.schedule.next(state.book.all(), Local::now())
    }

    async fn check(&self, watch_id: u64) {
        self.state().schedule.start(watch_id);
        self.check_watch(watch_id).await;
        self.state().schedule.finish();
    }

    async fn check_watch(&self, watch_id: u64) {
        let Some(watch) = self.state().book.get(watch_id).cloned() else {
            return;
        };
        match self.search(&watch).await {
            Ok(offers) => {
                self.state().schedule.source_answered(watch.source);
                let assessment = assess(&watch, offers, Local::now());
                if let Some((first_run, news)) = self.record(&watch, &assessment) {
                    self.alert(&watch, first_run, &news).await;
                }
            }
            Err(CheckError::Source(SourceError::Captcha)) => {
                let pause = self.state().schedule.captcha(watch.source, Local::now());
                let reason = format!(
                    "{}; {}",
                    failure_reason(watch.source.title(), &SourceError::Captcha),
                    pause_note(watch.source, pause)
                );
                self.fail(&watch, reason).await;
            }
            Err(error) => self.fail(&watch, error.reason(watch.source)).await,
        }
    }

    /// Пробная проверка паузу площадки не трогает: вы смотрите на окно и сами видите, что просит площадка.
    async fn try_out(&self, number: u64, watch: &Watch) {
        let status = match self.search(watch).await {
            Ok(offers) => TrialStatus::Done(assess(watch, offers, Local::now())),
            Err(error) => TrialStatus::Failed(error.reason(watch.source)),
        };
        self.state().trials.finish(number, status);
    }

    async fn search(&self, watch: &Watch) -> Result<Vec<Offer>, CheckError> {
        let Some(source) = self.sources.get(&watch.source) else {
            return Err(CheckError::Setup(format!(
                "{} ещё не подключено — нужна разовая настройка",
                watch.source.title()
            )));
        };
        let home = self.state().home;
        if watch.radius_km.is_some() && home.is_none() {
            return Err(CheckError::Setup(
                "не указан дом — укажите его координаты в «Настройках»".into(),
            ));
        }
        let area = watch.search_area(home);
        let found = match watch.link.as_deref() {
            Some(link) => {
                let link = watch.source.parse_link(link).map_err(CheckError::Setup)?;
                source.search_link(&link, area).await
            }
            None => source.search(&watch.query, area).await,
        };
        found.map_err(CheckError::Source)
    }

    /// Сверяет находки с памятью и запоминает их. `None` — слежку успели изменить или удалить, пока шла проверка.
    fn record(&self, watch: &Watch, assessment: &Assessment) -> Option<(bool, Vec<Deal>)> {
        let mut state = self.state();
        if !state.book.still_has(watch) {
            return None;
        }

        let first_run = state.notified.is_first_run(watch.id);
        let news = state.notified.take_news(watch.id, &assessment.suitable);
        state.feed.add(&news);
        state.journal.write(summary(watch, assessment, news.len()));
        let outcome = Outcome::Checked {
            scanned: assessment.scanned,
            comparable: assessment.comparable,
            market: assessment.market,
            suitable: assessment.suitable.len(),
            news: news.len(),
            hidden_suspicious: assessment.hidden_suspicious,
        };
        state.schedule.record(watch.id, Local::now(), outcome);
        state.save_deals(&self.storage);
        state.save_notified(&self.storage);
        Some((first_run, news))
    }

    /// Запоминает неудачу и сообщает о ней, только если до этого слежка работала.
    async fn fail(&self, watch: &Watch, reason: String) {
        let newly_failed = {
            let mut state = self.state();
            state
                .journal
                .write(format!("«{}»: проверка не удалась — {reason}", watch.name));
            state
                .schedule
                .record(watch.id, Local::now(), Outcome::Failed(reason.clone()))
        };
        if newly_failed && let Some(notifier) = &self.notifier {
            let alert = Alert::Failed { watch, reason: &reason };
            if let Err(error) = notifier.notify(alert).await {
                self.state()
                    .journal
                    .write(format!("Не удалось отправить уведомление: {error:#}"));
            }
        }
    }

    async fn alert(&self, watch: &Watch, first_run: bool, news: &[Deal]) {
        let Some(notifier) = &self.notifier else { return };
        let alert = if first_run {
            Alert::Started { watch, deals: news }
        } else if news.is_empty() {
            return;
        } else {
            Alert::NewDeals { watch, deals: news }
        };
        if let Err(error) = notifier.notify(alert).await {
            let mut state = self.state();
            for deal in news {
                state.notified.forget_offer(watch.id, &deal.offer.url);
            }
            state.save_notified(&self.storage);
            state.journal.write(format!(
                "Не удалось отправить уведомление, повторю при следующей проверке: {error:#}"
            ));
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn summary(watch: &Watch, assessment: &Assessment, news: usize) -> String {
    match assessment.market {
        Some(market) => format!(
            "«{}»: найдено {}, похожих {}, рынок {market}, выгодных {}, новых {news}, подозрительно дешёвых скрыто {}",
            watch.name,
            assessment.scanned,
            assessment.comparable,
            assessment.suitable.len(),
            assessment.hidden_suspicious
        ),
        None => format!(
            "«{}»: найдено {}, похожих {} — меньше {MIN_SAMPLE}, рыночную цену не посчитать; уточните запрос или категорию",
            watch.name, assessment.scanned, assessment.comparable
        ),
    }
}

/// Почему проверка не удалась: не с чем было идти на площадку — или площадка ответила ошибкой.
enum CheckError {
    Setup(String),
    Source(SourceError),
}

impl CheckError {
    fn reason(&self, source: SourceKind) -> String {
        match self {
            CheckError::Setup(reason) => reason.clone(),
            CheckError::Source(error) => failure_reason(source.title(), error),
        }
    }
}

fn pause_note(source: SourceKind, pause: Pause) -> String {
    match pause {
        Pause::Until(until) => format!("слежки {} подождут до {}", source.title(), until.format("%H:%M")),
        Pause::UntilManualCheck => format!(
            "это третья такая проверка подряд: слежки {} остановлены, пока вы не нажмёте «Проверить сейчас»",
            source.title()
        ),
    }
}

fn failure_reason(source: &str, error: &SourceError) -> String {
    match error {
        SourceError::Captcha => {
            format!("{source} просит подтвердить, что вы не робот — пройдите проверку в окне браузера программы")
        }
        SourceError::NotReady => format!(
            "страница {source} не открылась — посмотрите окно браузера программы, возможно, там проверка «я не робот»"
        ),
        SourceError::Failed(error) => format!("{error:#}"),
    }
}

/// Охотник целиком: с подставной площадкой, подставным Telegram и настоящими файлами во временной папке.
#[cfg(test)]
mod tests {
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;
    use crate::hunt::fixtures::{iphone_watch, market_offers, phone};
    use crate::marketplace::Area;
    use crate::offer::Offer;

    #[derive(Default)]
    struct ChangingSource {
        offers: StdMutex<Vec<Offer>>,
        captcha: AtomicBool,
        links: StdMutex<Vec<String>>,
    }

    impl ChangingSource {
        fn with(offers: Vec<Offer>) -> Arc<Self> {
            Arc::new(Self {
                offers: StdMutex::new(offers),
                ..Self::default()
            })
        }
    }

    #[async_trait]
    impl OfferSource for ChangingSource {
        fn name(&self) -> &'static str {
            "Ozon"
        }

        async fn search(&self, _query: &str, _area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
            if self.captcha.load(Ordering::SeqCst) {
                return Err(SourceError::Captcha);
            }
            Ok(self.offers.lock().unwrap().clone())
        }

        async fn search_link(&self, link: &reqwest::Url, area: Option<Area>) -> Result<Vec<Offer>, SourceError> {
            self.links.lock().unwrap().push(link.to_string());
            self.search("", area).await
        }
    }

    #[derive(Default)]
    struct RecordingNotifier {
        alerts: StdMutex<Vec<String>>,
        offline: AtomicBool,
    }

    #[async_trait]
    impl Notifier for RecordingNotifier {
        async fn notify(&self, alert: Alert<'_>) -> anyhow::Result<()> {
            if self.offline.load(Ordering::SeqCst) {
                anyhow::bail!("нет сети");
            }
            let text = match alert {
                Alert::Started { deals, .. } => format!("started:{}", deals.len()),
                Alert::NewDeals { deals, .. } => format!(
                    "new:{}",
                    deals
                        .iter()
                        .map(|d| d.offer.price.to_string())
                        .collect::<Vec<_>>()
                        .join(",")
                ),
                Alert::Failed { .. } => "failed".into(),
                Alert::Test => "test".into(),
            };
            self.alerts.lock().unwrap().push(text);
            Ok(())
        }
    }

    fn hunter_with(source: &Arc<ChangingSource>, notifier: &Arc<RecordingNotifier>, storage: &Storage) -> Hunter {
        Hunter::new(
            HashMap::from([
                (SourceKind::Ozon, Arc::clone(source) as Arc<dyn OfferSource>),
                (SourceKind::Avito, Arc::clone(source) as Arc<dyn OfferSource>),
            ]),
            Some(Arc::clone(notifier) as Arc<dyn Notifier>),
            storage.clone(),
            None,
        )
    }

    fn alerts(notifier: &RecordingNotifier) -> Vec<String> {
        notifier.alerts.lock().unwrap().clone()
    }

    /// Настоящие файлы во временной папке: тест заодно проверяет, что сохранённое читается. Папка удаляется,
    /// даже если тест упал.
    struct TempStorage(Storage);

    impl std::ops::Deref for TempStorage {
        type Target = Storage;

        fn deref(&self) -> &Storage {
            &self.0
        }
    }

    impl Drop for TempStorage {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(self.0.dir());
        }
    }

    fn storage(test: &str) -> TempStorage {
        let dir = std::env::temp_dir().join(format!("ozon-price-bot-hunt-{test}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        TempStorage(Storage::new(dir))
    }

    #[tokio::test]
    async fn first_check_reports_current_deals_then_only_new_ones() {
        let source = ChangingSource::with(market_offers());
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("flow");
        let hunter = hunter_with(&source, &notifier, &storage);
        let id = hunter.save_watch(iphone_watch()).unwrap();

        hunter.check(id).await;
        source.offers.lock().unwrap().push(phone(58_000));
        hunter.check(id).await;
        hunter.check(id).await;
        source.offers.lock().unwrap().push(phone(56_000));
        hunter.check(id).await;

        assert_eq!(
            *notifier.alerts.lock().unwrap(),
            ["started:0", "new:58000", "new:56000"]
        );
        assert_eq!(
            hunter.deals().iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [56_000, 58_000]
        );

        let reloaded = Hunter::new(HashMap::new(), None, storage.clone(), None);
        assert_eq!(reloaded.watches().len(), 1);
        assert_eq!(reloaded.deals().len(), 2);
    }

    #[tokio::test]
    async fn deal_not_delivered_to_telegram_is_sent_again_next_check() {
        let source = ChangingSource::with(market_offers());
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("retry");
        let hunter = hunter_with(&source, &notifier, &storage);
        let id = hunter.save_watch(iphone_watch()).unwrap();
        hunter.check(id).await;

        source.offers.lock().unwrap().push(phone(58_000));
        notifier.offline.store(true, Ordering::SeqCst);
        hunter.check(id).await;
        notifier.offline.store(false, Ordering::SeqCst);
        hunter.check(id).await;
        hunter.check(id).await;

        assert_eq!(alerts(&notifier), ["started:0", "new:58000"]);
        assert_eq!(hunter.deals().len(), 1);
    }

    #[tokio::test]
    async fn failure_is_reported_once_and_again_only_after_recovery() {
        let source = ChangingSource::with(market_offers());
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("failure");
        let hunter = hunter_with(&source, &notifier, &storage);
        let id = hunter.save_watch(iphone_watch()).unwrap();

        source.captcha.store(true, Ordering::SeqCst);
        hunter.check(id).await;
        hunter.check(id).await;
        source.captcha.store(false, Ordering::SeqCst);
        hunter.check(id).await;
        source.captcha.store(true, Ordering::SeqCst);
        hunter.check(id).await;

        assert_eq!(alerts(&notifier), ["failed", "started:0", "failed"]);
        assert!(matches!(
            hunter.run_status(id).map(|run| run.outcome),
            Some(Outcome::Failed(reason)) if reason.contains("пройдите проверку в окне браузера программы")
        ));
    }

    #[tokio::test]
    async fn captchas_in_a_row_stop_the_source_until_the_button_and_success_lifts_it() {
        let source = ChangingSource::with(market_offers());
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("captcha-pause");
        let hunter = hunter_with(&source, &notifier, &storage);
        let id = hunter.save_watch(iphone_watch()).unwrap();
        let reason = |hunter: &Hunter| match hunter.run_status(id).map(|run| run.outcome) {
            Some(Outcome::Failed(reason)) => reason,
            other => panic!("ждали сбой, а не {other:?}"),
        };

        source.captcha.store(true, Ordering::SeqCst);
        hunter.check(id).await;
        assert!(matches!(hunter.source_pause(SourceKind::Ozon), Some(Pause::Until(_))));
        assert!(reason(&hunter).contains("пройдите проверку в окне браузера программы; слежки Ozon подождут до"));
        hunter.check(id).await;
        hunter.check(id).await;
        assert_eq!(hunter.source_pause(SourceKind::Ozon), Some(Pause::UntilManualCheck));
        assert!(reason(&hunter).contains("остановлены, пока вы не нажмёте «Проверить сейчас»"));
        assert_eq!(alerts(&notifier), ["failed"], "о сбое — одно сообщение");

        hunter.run_now(id);
        assert_eq!(hunter.next_to_check(), Some(id), "кнопка обходит паузу");
        source.captcha.store(false, Ordering::SeqCst);
        hunter.check(id).await;
        assert_eq!(hunter.source_pause(SourceKind::Ozon), None);
    }

    #[tokio::test]
    async fn cheaper_again_offer_replaces_its_old_entry_in_deals() {
        let source = ChangingSource::with(market_offers());
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("replace");
        let hunter = hunter_with(&source, &notifier, &storage);
        let id = hunter.save_watch(iphone_watch()).unwrap();
        hunter.check(id).await;

        let same_listing = |price| Offer {
            url: "https://example.com/same".into(),
            ..phone(price)
        };
        source.offers.lock().unwrap().push(same_listing(58_000));
        hunter.check(id).await;
        source
            .offers
            .lock()
            .unwrap()
            .retain(|o| o.url != "https://example.com/same");
        source.offers.lock().unwrap().push(same_listing(55_000));
        hunter.check(id).await;

        assert_eq!(alerts(&notifier), ["started:0", "new:58000", "new:55000"]);
        assert_eq!(
            hunter.deals().iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [55_000]
        );
    }

    #[tokio::test]
    async fn setting_home_retries_watches_that_waited_for_it() {
        let source = ChangingSource::with(Vec::new());
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("home");
        let hunter = Hunter::new(
            HashMap::from([(SourceKind::Avito, Arc::clone(&source) as Arc<dyn OfferSource>)]),
            Some(Arc::clone(&notifier) as Arc<dyn Notifier>),
            storage.clone(),
            Some(GeoPoint { lat: 55.75, lon: 37.62 }),
        );
        let id = hunter
            .save_watch(Watch {
                source: SourceKind::Avito,
                radius_km: Some(5),
                ..iphone_watch()
            })
            .unwrap();
        hunter.set_home(None);
        hunter.check(id).await;
        assert!(matches!(
            hunter.run_status(id).map(|run| run.outcome),
            Some(Outcome::Failed(_))
        ));

        hunter.set_home(Some(GeoPoint { lat: 55.75, lon: 37.62 }));

        assert!(hunter.run_status(id).is_none());
        assert_eq!(hunter.next_to_check(), Some(id));
    }

    #[test]
    fn saving_already_deleted_watch_is_rejected() {
        let storage = storage("deleted");
        let hunter = Hunter::new(HashMap::new(), None, storage.clone(), None);
        let id = hunter.save_watch(iphone_watch()).unwrap();
        hunter.delete_watch(id);

        let problems = hunter.save_watch(Watch { id, ..iphone_watch() }).unwrap_err();

        assert!(problems[0].contains("уже удалили"));
        assert!(hunter.watches().is_empty());
    }

    #[tokio::test]
    async fn unconnected_source_is_reported_as_failure() {
        let storage = storage("avito");
        let hunter = Hunter::new(HashMap::new(), None, storage.clone(), None);
        let id = hunter
            .save_watch(Watch {
                source: SourceKind::Avito,
                ..iphone_watch()
            })
            .unwrap();

        hunter.check(id).await;

        assert!(matches!(
            hunter.run_status(id).map(|run| run.outcome),
            Some(Outcome::Failed(reason)) if reason.contains("Авито ещё не подключено")
        ));
    }

    #[tokio::test]
    async fn trial_shows_what_would_be_found_without_saving_or_notifying() {
        let mut offers = market_offers();
        offers.push(phone(58_000));
        let source = ChangingSource::with(offers);
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("trial");
        let hunter = hunter_with(&source, &notifier, &storage);

        hunter.try_watch(iphone_watch()).unwrap();
        assert!(matches!(hunter.trial(), Some(TrialStatus::Queued)));
        hunter.check_pending().await;

        let Some(TrialStatus::Done(assessment)) = hunter.trial() else {
            panic!("пробная проверка должна закончиться итогом");
        };
        assert_eq!(assessment.market, Some(MarketPrice::Whole(72_000)));
        assert_eq!(
            assessment.suitable.iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [58_000]
        );
        assert!(alerts(&notifier).is_empty(), "пробная проверка ничего не отправляет");
        assert!(hunter.deals().is_empty() && hunter.watches().is_empty());
        assert!(
            !storage.dir().join(feed::FILE).exists() && !storage.dir().join(memory::FILE).exists(),
            "и ничего не записывает"
        );
    }

    #[tokio::test]
    async fn watch_with_link_searches_by_it_and_judges_by_its_words() {
        let mut offers = market_offers();
        offers.push(phone(58_000));
        let source = ChangingSource::with(offers);
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("link");
        let hunter = hunter_with(&source, &notifier, &storage);
        let link = "https://ozon.ru/category/smartfony-15502/apple-26303000/?delivery=2";
        let id = hunter
            .save_watch(Watch {
                link: Some(link.into()),
                ..iphone_watch()
            })
            .unwrap();

        hunter.check(id).await;

        assert_eq!(
            *source.links.lock().unwrap(),
            ["https://www.ozon.ru/category/smartfony-15502/apple-26303000/?delivery=2"]
        );
        assert_eq!(
            hunter.deals().iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [58_000],
            "чехол и Pro отсеяны словами слежки, как и без ссылки"
        );
    }

    #[tokio::test]
    async fn dismissed_deal_never_returns_and_stop_word_keeps_the_watch_going() {
        let import = |price| Offer {
            url: "https://example.com/import".into(),
            ..Offer::sample("Apple iPhone 15 128GB серый импорт", price)
        };
        let mut offers = market_offers();
        offers.extend([phone(58_000), import(57_000)]);
        let source = ChangingSource::with(offers);
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("dismiss");
        let hunter = hunter_with(&source, &notifier, &storage);
        let id = hunter.save_watch(iphone_watch()).unwrap();
        hunter.check(id).await;

        let words = hunter.dismiss(id, "https://example.com/import", Miss::Fake);

        assert_eq!(words, ["серый", "импорт"], "«apple» и «128gb» есть и в хорошей находке");
        assert_eq!(
            hunter.deals().iter().map(|d| d.offer.price).collect::<Vec<_>>(),
            [58_000]
        );
        assert!(
            hunter
                .journal()
                .iter()
                .any(|entry| entry.text.contains("мимо, подделка"))
        );

        source
            .offers
            .lock()
            .unwrap()
            .retain(|o| o.url != "https://example.com/import");
        source.offers.lock().unwrap().push(import(50_000));
        hunter.check(id).await;
        assert_eq!(hunter.deals().len(), 1, "отмеченная «мимо» не вернулась и подешевев");

        hunter.add_stop_word(id, "импорт");
        hunter.check(id).await;
        assert_eq!(hunter.watches()[0].stop_words, ["импорт"]);
        assert_eq!(
            alerts(&notifier),
            ["started:2"],
            "стоп-слово не начинает слежку заново: второго «запущена» нет"
        );
        assert!(hunter.dismiss(id, "https://example.com/import", Miss::Sold).is_empty());
    }

    #[tokio::test]
    async fn failed_trial_says_why() {
        let source = ChangingSource::with(market_offers());
        source.captcha.store(true, Ordering::SeqCst);
        let notifier = Arc::new(RecordingNotifier::default());
        let storage = storage("trial-failed");
        let hunter = hunter_with(&source, &notifier, &storage);

        hunter.try_watch(iphone_watch()).unwrap();
        hunter.check_pending().await;

        assert!(matches!(
            hunter.trial(),
            Some(TrialStatus::Failed(reason)) if reason.contains("пройдите проверку в окне браузера программы")
        ));
        assert!(alerts(&notifier).is_empty());
    }

    #[test]
    fn invalid_watch_is_not_tried() {
        let storage = storage("trial-invalid");
        let hunter = Hunter::new(HashMap::new(), None, storage.clone(), None);
        assert_eq!(hunter.try_watch(Watch::new_draft()).unwrap_err().len(), 2);
        assert!(hunter.trial().is_none());
    }

    #[test]
    fn invalid_watch_is_not_saved() {
        let storage = storage("invalid");
        let hunter = Hunter::new(HashMap::new(), None, storage.clone(), None);
        let problems = hunter.save_watch(Watch::new_draft()).unwrap_err();
        assert_eq!(problems.len(), 2);
        assert!(hunter.watches().is_empty());
    }
}
