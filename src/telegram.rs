mod buttons;
mod render;
mod self_check;

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use chrono::Local;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile, LinkPreviewOptions, MessageId, ParseMode, UserId};
use tokio::sync::watch;

use self::buttons::{Answer, Answers, Press, Pressed, deal_key};
use crate::export;
use crate::health::ParserCheck;
use crate::hunt::{
    Alert, Category, Deal, Hunter, Notifier, OffersProgress, PriceMemory, RunStatus, SourceReport, Watch,
    best_shop_offer, hunt_offers,
};
use crate::marketplace::ozon::Ozon;
use crate::marketplace::{SourceError, SourceKind};
use crate::search::{Findings, OfferSearch, SearchRequest};

const WITHOUT_LINK_PREVIEW: LinkPreviewOptions = LinkPreviewOptions {
    is_disabled: true,
    url: None,
    prefer_small_media: false,
    prefer_large_media: false,
    show_above_text: false,
};

pub struct TelegramSettings {
    pub token: String,
    pub allowed_users: HashSet<u64>,
    /// Кому писать о находках и самопроверке — ID из настроек программы или первый из `ALLOWED_USERS`.
    pub owner: Option<u64>,
    pub top: usize,
}

/// Отвечает в чате: ищет по запросу, управляет слежками охотника и рассказывает, как дела у браузера и площадок.
struct ChatBot {
    search: OfferSearch,
    memory: Arc<PriceMemory>,
    hunter: Arc<Hunter>,
    ozon: Arc<Ozon>,
    allowed_users: HashSet<u64>,
    top: usize,
    answers: Mutex<Answers>,
}

/// Уведомления о находках слежек — владельцу бота в личные сообщения.
pub struct TelegramAlerts {
    bot: Bot,
    owner: ChatId,
}

impl TelegramAlerts {
    pub fn new(bot: Bot, owner: u64) -> Self {
        Self {
            bot,
            owner: ChatId::from(UserId(owner)),
        }
    }
}

#[async_trait]
impl Notifier for TelegramAlerts {
    async fn notify(&self, alert: Alert<'_>) -> anyhow::Result<()> {
        let message = self
            .bot
            .send_message(self.owner, render::alert(&alert))
            .parse_mode(ParseMode::Html)
            .link_preview_options(WITHOUT_LINK_PREVIEW);
        match render::alert_keyboard(&alert) {
            Some(keyboard) => message.reply_markup(keyboard).await?,
            None => message.await?,
        };
        Ok(())
    }
}

pub async fn run(
    bot: Bot,
    settings: TelegramSettings,
    search: OfferSearch,
    memory: Arc<PriceMemory>,
    hunter: Arc<Hunter>,
    ozon: Arc<Ozon>,
    parser_check: Option<ParserCheck>,
) {
    if let Some(check) = parser_check {
        tokio::spawn(self_check::watch(bot.clone(), settings.owner.map(UserId), check));
    }
    // Меню команд у поля ввода: без него о /watches можно узнать только из справки.
    if let Err(error) = bot.set_my_commands(render::commands()).await {
        log::warn!("Telegram не принял меню команд: {error}");
    }
    let chat_bot = Arc::new(ChatBot {
        search,
        memory,
        hunter,
        ozon,
        allowed_users: settings.allowed_users,
        top: settings.top,
        answers: Mutex::new(Answers::new()),
    });
    let handler = dptree::entry()
        .branch(
            Update::filter_message().endpoint(|bot: Bot, message: Message, chat_bot: Arc<ChatBot>| async move {
                chat_bot.handle(&bot, &message).await
            }),
        )
        .branch(Update::filter_callback_query().endpoint(
            |bot: Bot, query: CallbackQuery, chat_bot: Arc<ChatBot>| async move { chat_bot.press(&bot, &query).await },
        ));

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![chat_bot])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

impl ChatBot {
    async fn handle(&self, bot: &Bot, message: &Message) -> ResponseResult<()> {
        let Some(text) = message.text() else { return Ok(()) };
        let chat = message.chat.id;
        let user_id = message.from.as_ref().map_or(0, |user| user.id.0);
        let answer = match reply_to(text, user_id, &self.allowed_users) {
            Reply::AccessDenied { user_id } => render::access_denied(user_id),
            Reply::Help => render::HELP.to_string(),
            Reply::Search(request) => return self.answer(bot, chat, &request).await,
            Reply::Status => {
                let reports: Vec<(SourceKind, SourceReport)> = SourceKind::ALL
                    .into_iter()
                    .map(|source| (source, self.hunter.source_report(source)))
                    .collect();
                render::status(self.ozon.site_status().await, &reports)
            }
            Reply::Watches => {
                let watches: Vec<(Watch, Option<RunStatus>)> = self
                    .hunter
                    .watches()
                    .into_iter()
                    .map(|watch| {
                        let status = self.hunter.run_status(watch.id);
                        (watch, status)
                    })
                    .collect();
                render::watch_list(&watches)
            }
            Reply::Watch(action, watch_id) => self.act(action, watch_id),
            Reply::WhichWatch(action) => render::which_watch(action),
        };
        bot.send_message(chat, answer)
            .parse_mode(ParseMode::Html)
            .link_preview_options(WITHOUT_LINK_PREVIEW)
            .await?;
        Ok(())
    }

    /// Те же действия, что кнопки в окне программы: выключить, включить, проверить сейчас.
    fn act(&self, action: WatchAction, watch_id: u64) -> String {
        let Some(watch) = self.hunter.watches().into_iter().find(|watch| watch.id == watch_id) else {
            return render::no_such_watch(watch_id);
        };
        match action {
            WatchAction::Pause => self.hunter.set_enabled(watch_id, false),
            WatchAction::Resume => self.hunter.set_enabled(watch_id, true),
            WatchAction::Check => self.hunter.run_now(watch_id),
        }
        render::watch_done(action, &watch)
    }

    async fn answer(&self, bot: &Bot, chat: ChatId, request: &SearchRequest) -> ResponseResult<()> {
        let sources = self.search.source_names(request);
        let status = bot
            .send_message(chat, render::searching(&request.query, &sources))
            .await?;

        match self.hunt_showing_progress(bot, chat, status.id, request).await {
            Ok(mut findings) => {
                for (source, error) in &findings.failed {
                    log::warn!("поиск «{}»: {} не ответил — {error:#}", request.query, source.title());
                }
                if let Some((source, price)) = best_shop_offer(&findings).map(|best| (best.source, best.offer.price)) {
                    let low = self.memory.note(request, Category::Any, source, price, Local::now());
                    findings.price_note = low.map(|low| low.says(price));
                }
                let table = (!findings.suitable.is_empty())
                    .then(|| export::xlsx(&findings.suitable, request.per_unit))
                    .and_then(|table| {
                        table
                            .map_err(|error| log::warn!("таблица «{}» не собралась: {error:#}", request.query))
                            .ok()
                    });
                let (text, keyboard) = {
                    let mut answers = self.answers.lock().unwrap();
                    let key = answers.add(Answer::new(request.clone(), findings));
                    answers.get_mut(key).map(|answer| answer.show(key, self.top)).unwrap()
                };
                bot.edit_message_text(chat, status.id, text)
                    .parse_mode(ParseMode::Html)
                    .link_preview_options(WITHOUT_LINK_PREVIEW)
                    .reply_markup(keyboard)
                    .await?;
                if let Some(table) = table {
                    let table = InputFile::memory(table).file_name(render::table_file_name(&request.query));
                    bot.send_document(chat, table).await?;
                }
            }
            Err((source, error)) => {
                let source = source.title();
                log::warn!("поиск «{}» на {source} не удался: {error:#}", request.query);
                bot.edit_message_text(chat, status.id, render::failure(&error, source))
                    .await?;
            }
        }
        Ok(())
    }

    /// Ход поиска правит сообщение «Ищу…». Последний шаг дописывается до возврата: иначе правка могла бы лечь поверх
    /// готового ответа. Не прошедшая правка не мешает ответу.
    async fn hunt_showing_progress(
        &self,
        bot: &Bot,
        chat: ChatId,
        status: MessageId,
        request: &SearchRequest,
    ) -> Result<Findings, (SourceKind, SourceError)> {
        let (progress, mut shown) = watch::channel(String::new());
        let hunting = async move {
            let report = |step: OffersProgress| {
                let text = render::search_progress(&request.query, &step);
                progress.send_if_modified(|shown| {
                    let changed = *shown != text;
                    *shown = text;
                    changed
                });
            };
            hunt_offers(&self.search, request, report).await
        };
        let showing = async {
            while shown.changed().await.is_ok() {
                let text = shown.borrow_and_update().clone();
                if let Err(error) = bot.edit_message_text(chat, status, text).await {
                    log::debug!("ход поиска «{}» не показан: {error}", request.query);
                }
            }
        };
        tokio::join!(hunting, showing).0
    }

    /// Нажатие надо подтвердить в любом случае: без ответа Telegram крутит на кнопке часики.
    async fn press(&self, bot: &Bot, query: &CallbackQuery) -> ResponseResult<()> {
        let note = match buttons::pressed(query.data.as_deref(), query.from.id.0, &self.allowed_users) {
            Pressed::Denied => Some(render::BUTTON_DENIED.to_string()),
            Pressed::Unknown => Some(render::BUTTON_STALE.to_string()),
            Pressed::Button(press) => match query.regular_message() {
                Some(message) => self.act_on(bot, message, press).await.unwrap_or_else(|error| {
                    log::warn!("кнопка {}: {error}", press.data());
                    Some(render::BUTTON_FAILED.to_string())
                }),
                None => Some(render::BUTTON_STALE.to_string()),
            },
        };
        let confirmation = bot.answer_callback_query(query.id.clone());
        match note {
            Some(note) => confirmation.text(note).await?,
            None => confirmation.await?,
        };
        Ok(())
    }

    /// Что сделать по кнопке; ответ — короткая всплывающая подсказка, если сообщение в чате само не изменилось.
    async fn act_on(&self, bot: &Bot, message: &Message, press: Press) -> ResponseResult<Option<String>> {
        match press {
            Press::More(key) | Press::ByDiscount(key) | Press::GoodRating(key) => {
                let shown = self.answers.lock().unwrap().get_mut(key).map(|answer| {
                    answer.apply(press);
                    answer.show(key, self.top)
                });
                let Some((text, keyboard)) = shown else {
                    return Ok(Some(render::BUTTON_STALE.to_string()));
                };
                bot.edit_message_text(message.chat.id, message.id, text)
                    .parse_mode(ParseMode::Html)
                    .link_preview_options(WITHOUT_LINK_PREVIEW)
                    .reply_markup(keyboard)
                    .await?;
                Ok(None)
            }
            Press::Watch(key) => {
                let request = self
                    .answers
                    .lock()
                    .unwrap()
                    .get_mut(key)
                    .map(|answer| answer.request.clone());
                Ok(Some(request.map_or(render::BUTTON_STALE.to_string(), |request| {
                    match self.hunter.save_watch(Watch::for_request(&request)) {
                        Ok(_) => render::watch_saved(&request.query),
                        Err(problems) => render::watch_refused(&problems),
                    }
                })))
            }
            Press::Miss { watch_id, deal } => {
                let Some(deal) = self.deal(watch_id, deal) else {
                    return Ok(Some(render::DEAL_GONE.to_string()));
                };
                bot.send_message(message.chat.id, render::miss_question(&deal))
                    .parse_mode(ParseMode::Html)
                    .reply_markup(render::miss_keyboard(&deal))
                    .await?;
                Ok(None)
            }
            Press::Reason { watch_id, deal, miss } => {
                let Some(deal) = self.deal(watch_id, deal) else {
                    return Ok(Some(render::DEAL_GONE.to_string()));
                };
                let stop_words = self.hunter.dismiss(watch_id, &deal.offer.url, miss);
                bot.edit_message_text(message.chat.id, message.id, render::missed(&deal, miss, &stop_words))
                    .parse_mode(ParseMode::Html)
                    .await?;
                Ok(None)
            }
        }
    }

    fn deal(&self, watch_id: u64, key: u64) -> Option<Deal> {
        self.hunter
            .deals()
            .into_iter()
            .find(|deal| deal.watch_id == watch_id && deal_key(&deal.offer.url) == key)
    }
}

/// Что ответить на сообщение. Решение отделено от отправки, чтобы правила доступа и справки проверялись без Telegram.
#[derive(Debug, PartialEq)]
enum Reply {
    AccessDenied {
        user_id: u64,
    },
    Help,
    Search(SearchRequest),
    Status,
    Watches,
    Watch(WatchAction, u64),
    /// Команда слежки без номера — подсказать, где его взять.
    WhichWatch(WatchAction),
}

/// Что сделать со слежкой по команде из Telegram.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WatchAction {
    Pause,
    Resume,
    Check,
}

impl WatchAction {
    fn command(self) -> &'static str {
        match self {
            WatchAction::Pause => "/pause",
            WatchAction::Resume => "/resume",
            WatchAction::Check => "/check",
        }
    }
}

/// Чужим — отказ с их ID (его вписывают в `ALLOWED_USERS`). Команды слежек — по номеру из `/watches`;
/// остальные команды и сообщения без слов для поиска — справка: «до 5000» или «-чехол» сами по себе искать нечего.
fn reply_to(text: &str, user_id: u64, allowed_users: &HashSet<u64>) -> Reply {
    if !allowed_users.contains(&user_id) {
        return Reply::AccessDenied { user_id };
    }
    if let Some(command) = text.strip_prefix('/') {
        return command_reply(command);
    }
    let request = SearchRequest::parse(text);
    if request.query.is_empty() {
        Reply::Help
    } else {
        Reply::Search(request)
    }
}

/// `pause 3`, `pause@имя_бота 3` (так Telegram пишет команду из меню в группе), `pause №3`.
fn command_reply(command: &str) -> Reply {
    let mut words = command.split_whitespace();
    let name = words.next().unwrap_or_default();
    let name = name.split('@').next().unwrap_or_default();
    let action = match name {
        "status" => return Reply::Status,
        "watches" => return Reply::Watches,
        "pause" => WatchAction::Pause,
        "resume" => WatchAction::Resume,
        "check" => WatchAction::Check,
        _ => return Reply::Help,
    };
    match words.next().map(|number| number.trim_start_matches(['№', '#']).parse()) {
        Some(Ok(watch_id)) => Reply::Watch(action, watch_id),
        _ => Reply::WhichWatch(action),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OWNER: u64 = 123_456_789;

    fn reply(text: &str, user_id: u64) -> Reply {
        reply_to(text, user_id, &HashSet::from([OWNER]))
    }

    #[test]
    fn stranger_is_refused_with_own_id_whatever_they_write() {
        assert_eq!(reply("/start", 42), Reply::AccessDenied { user_id: 42 });
        assert_eq!(reply("наушники sony", 42), Reply::AccessDenied { user_id: 42 });
    }

    #[test]
    fn commands_and_messages_without_words_get_help() {
        for text in [
            "/start",
            "/help наушники",
            "до 5000",
            "-чехол",
            "рейтинг от 4.5 по скидке",
        ] {
            assert_eq!(reply(text, OWNER), Reply::Help, "{text}");
        }
    }

    #[test]
    fn watch_commands_take_the_number_from_the_list() {
        assert_eq!(reply("/watches", OWNER), Reply::Watches);
        assert_eq!(reply("/status", OWNER), Reply::Status);
        assert_eq!(reply("/pause 3", OWNER), Reply::Watch(WatchAction::Pause, 3));
        assert_eq!(reply("/resume №3", OWNER), Reply::Watch(WatchAction::Resume, 3));
        assert_eq!(reply("/check@hunt_bot 12", OWNER), Reply::Watch(WatchAction::Check, 12));
        assert_eq!(reply("/pause", OWNER), Reply::WhichWatch(WatchAction::Pause));
        assert_eq!(reply("/check наушники", OWNER), Reply::WhichWatch(WatchAction::Check));
        assert_eq!(
            reply("/pause 3", 42),
            Reply::AccessDenied { user_id: 42 },
            "чужим — ни списка, ни паузы"
        );
    }

    #[test]
    fn anything_with_words_is_searched_as_written() {
        assert_eq!(
            reply("наушники sony до 5000 по скидке", OWNER),
            Reply::Search(SearchRequest::parse("наушники sony до 5000 по скидке"))
        );
    }
}
