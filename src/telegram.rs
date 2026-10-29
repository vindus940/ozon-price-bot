mod render;
mod self_check;

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile, LinkPreviewOptions, ParseMode, UserId};

use crate::export;
use crate::health::ParserCheck;
use crate::hunt::{Alert, Hunter, Notifier, RunStatus, SourceReport, Watch};
use crate::marketplace::SourceKind;
use crate::marketplace::ozon::Ozon;
use crate::search::{OfferSearch, SearchRequest};

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
    hunter: Arc<Hunter>,
    ozon: Arc<Ozon>,
    allowed_users: HashSet<u64>,
    top: usize,
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
        self.bot
            .send_message(self.owner, render::alert(&alert))
            .parse_mode(ParseMode::Html)
            .link_preview_options(WITHOUT_LINK_PREVIEW)
            .await?;
        Ok(())
    }
}

pub async fn run(
    bot: Bot,
    settings: TelegramSettings,
    search: OfferSearch,
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
        hunter,
        ozon,
        allowed_users: settings.allowed_users,
        top: settings.top,
    });
    let handler = Update::filter_message().endpoint(|bot: Bot, message: Message, chat_bot: Arc<ChatBot>| async move {
        chat_bot.handle(&bot, &message).await
    });

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
        let source = self.search.source_name();
        let status = bot
            .send_message(chat, render::searching(&request.query, source))
            .await?;

        match self.search.find(request).await {
            Ok(findings) => {
                bot.edit_message_text(chat, status.id, render::findings(request, &findings, self.top))
                    .parse_mode(ParseMode::Html)
                    .link_preview_options(WITHOUT_LINK_PREVIEW)
                    .await?;
                if !findings.suitable.is_empty() {
                    let table = InputFile::memory(export::csv(&findings.suitable, request.per_unit))
                        .file_name(render::table_file_name(&request.query, source));
                    bot.send_document(chat, table).await?;
                }
            }
            Err(error) => {
                log::warn!("поиск «{}» на {source} не удался: {error:#}", request.query);
                bot.edit_message_text(chat, status.id, render::failure(&error, source))
                    .await?;
            }
        }
        Ok(())
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
