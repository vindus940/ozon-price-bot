use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

const HTTP_TIMEOUT: Duration = Duration::from_secs(5);
const COMMAND_ID: u64 = 1;

pub(super) struct DevTools {
    http: reqwest::Client,
    endpoint: String,
}

pub struct Tab {
    socket: String,
}

#[derive(Deserialize)]
struct Target {
    #[serde(rename = "type")]
    kind: String,
    url: String,
    #[serde(rename = "webSocketDebuggerUrl")]
    socket: Option<String>,
}

impl DevTools {
    pub(super) fn new(port: u16) -> Result<Self> {
        let http = reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()?;
        Ok(Self {
            http,
            endpoint: format!("http://127.0.0.1:{port}"),
        })
    }

    pub(super) async fn is_available(&self) -> bool {
        self.http
            .get(format!("{}/json/version", self.endpoint))
            .send()
            .await
            .is_ok_and(|response| response.status().is_success())
    }

    pub(super) async fn find_tab(&self, url_prefix: &str) -> Result<Option<Tab>> {
        let targets: Vec<Target> = self
            .http
            .get(format!("{}/json/list", self.endpoint))
            .send()
            .await?
            .json()
            .await?;
        Ok(targets
            .into_iter()
            .find(|target| target.kind == "page" && target.url.starts_with(url_prefix))
            .and_then(Tab::from_target))
    }

    pub(super) async fn open_tab(&self, url: &str) -> Result<Tab> {
        let target: Target = self
            .http
            .put(format!("{}/json/new?{url}", self.endpoint))
            .send()
            .await?
            .json()
            .await?;
        Tab::from_target(target).context("браузер не дал доступ к новой вкладке")
    }
}

impl Tab {
    fn from_target(target: Target) -> Option<Self> {
        target.socket.map(|socket| Self { socket })
    }

    /// Выполняет выражение во вкладке — как вставку в консоль — и дожидается результата, даже асинхронного.
    pub async fn evaluate<T: DeserializeOwned>(&self, expression: &str) -> Result<T> {
        let reply = self
            .command(
                "Runtime.evaluate",
                json!({ "expression": expression, "awaitPromise": true, "returnByValue": true }),
            )
            .await?;
        evaluation_result(reply)
    }

    /// Скрытую вкладку Edge со временем замораживает: синхронный код в ней ещё выполняется, а таймеры и ответы `fetch`
    /// не приходят, и поиск Ozon висел до тайм-аута. Команда возвращает вкладку к обычной работе, окно не показывая.
    pub async fn wake(&self) -> Result<()> {
        let reply = self
            .command("Page.setWebLifecycleState", json!({ "state": "active" }))
            .await?;
        if let Some(error) = reply.get("error") {
            bail!("браузер не разбудил вкладку: {error}");
        }
        Ok(())
    }

    /// Открывает адрес во вкладке, как если бы его вставили в адресную строку.
    /// Возвращается, когда переход начался; дождаться загрузки — забота вызывающего.
    pub async fn navigate(&self, url: &str) -> Result<()> {
        let reply = self.command("Page.navigate", json!({ "url": url })).await?;
        if let Some(error) = reply.get("error") {
            bail!("браузер отклонил переход: {error}");
        }
        match reply["result"]["errorText"].as_str() {
            Some(error) if !error.is_empty() => bail!("страница не открылась: {error}"),
            _ => Ok(()),
        }
    }

    async fn command(&self, method: &str, params: Value) -> Result<Value> {
        let (mut connection, _) = connect_async(self.socket.as_str())
            .await
            .context("не удалось подключиться к вкладке")?;
        let command = json!({ "id": COMMAND_ID, "method": method, "params": params });
        connection.send(Message::Text(command.to_string().into())).await?;

        while let Some(message) = connection.next().await {
            let Message::Text(text) = message? else { continue };
            let reply: Value = serde_json::from_str(text.as_str())?;
            if reply["id"] == COMMAND_ID {
                return Ok(reply);
            }
        }
        bail!("браузер закрыл соединение")
    }
}

fn evaluation_result<T: DeserializeOwned>(mut reply: Value) -> Result<T> {
    if let Some(error) = reply.get("error") {
        bail!("браузер отклонил команду: {error}");
    }
    let mut result = reply["result"].take();
    if let Some(details) = result.get("exceptionDetails") {
        let reason = details["exception"]["description"]
            .as_str()
            .or(details["text"].as_str())
            .unwrap_or("неизвестная ошибка");
        bail!("ошибка в скрипте: {reason}");
    }
    serde_json::from_value(result["result"]["value"].take()).context("скрипт вернул данные неожиданного вида")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returned_value_is_deserialized() {
        let reply = json!({ "id": 1, "result": { "result": { "type": "boolean", "value": true } } });
        assert!(evaluation_result::<bool>(reply).unwrap());
    }

    #[test]
    fn script_exception_becomes_error() {
        let reply = json!({ "id": 1, "result": {
            "result": { "type": "object" },
            "exceptionDetails": { "text": "Uncaught", "exception": { "description": "TypeError: x is undefined" } }
        } });
        let error = evaluation_result::<Value>(reply).unwrap_err().to_string();
        assert!(error.contains("TypeError: x is undefined"), "{error}");
    }

    #[test]
    fn protocol_error_becomes_error() {
        let reply = json!({ "id": 1, "error": { "code": -32000, "message": "Cannot find context" } });
        assert!(
            evaluation_result::<Value>(reply)
                .unwrap_err()
                .to_string()
                .contains("Cannot find context")
        );
    }
}
