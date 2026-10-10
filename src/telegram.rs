mod render;
mod self_check;

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use teloxide::prelude::*;
use teloxide::types::{ChatId, InputFile, LinkPreviewOptions, ParseMode, UserId};

use crate::export;
use crate::health::ParserCheck;
use crate::hunt::{Alert, Notifier};
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

struct SearchBot {
    search: OfferSearch,
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

pub async fn run(bot: Bot, settings: TelegramSettings, search: OfferSearch, parser_check: Option<ParserCheck>) {
    if let Some(check) = parser_check {
        tokio::spawn(self_check::watch(bot.clone(), settings.owner.map(UserId), check));
    }
    let search_bot = Arc::new(SearchBot {
        search,
        allowed_users: settings.allowed_users,
        top: settings.top,
    });
    let handler =
        Update::filter_message().endpoint(|bot: Bot, message: Message, search_bot: Arc<SearchBot>| async move {
            search_bot.handle(&bot, &message).await
        });

    Dispatcher::builder(bot, handler)
        .dependencies(dptree::deps![search_bot])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

impl SearchBot {
    async fn handle(&self, bot: &Bot, message: &Message) -> ResponseResult<()> {
        let Some(text) = message.text() else { return Ok(()) };
        let chat = message.chat.id;
        let user_id = message.from.as_ref().map_or(0, |user| user.id.0);
        match reply_to(text, user_id, &self.allowed_users) {
            Reply::AccessDenied { user_id } => {
                bot.send_message(chat, render::access_denied(user_id)).await?;
            }
            Reply::Help => {
                bot.send_message(chat, render::HELP).parse_mode(ParseMode::Html).await?;
            }
            Reply::Search(request) => self.answer(bot, chat, &request).await?,
        }
        Ok(())
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
    AccessDenied { user_id: u64 },
    Help,
    Search(SearchRequest),
}

/// Чужим — отказ с их ID (его вписывают в `ALLOWED_USERS`). Команды и сообщения без слов для поиска —
/// справка: «до 5000» или «-чехол» сами по себе искать нечего.
fn reply_to(text: &str, user_id: u64, allowed_users: &HashSet<u64>) -> Reply {
    if !allowed_users.contains(&user_id) {
        return Reply::AccessDenied { user_id };
    }
    let request = SearchRequest::parse(text);
    if text.starts_with('/') || request.query.is_empty() {
        Reply::Help
    } else {
        Reply::Search(request)
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
    fn anything_with_words_is_searched_as_written() {
        assert_eq!(
            reply("наушники sony до 5000 по скидке", OWNER),
            Reply::Search(SearchRequest::parse("наушники sony до 5000 по скидке"))
        );
    }
}
