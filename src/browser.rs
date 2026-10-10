mod devtools;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::wait;
use devtools::DevTools;
pub use devtools::Tab;

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(15);

/// Отдельный браузер для живых тестов: свой порт и профиль в `target`, чтобы не трогать окно программы.
#[cfg(test)]
pub fn test_browser() -> std::sync::Arc<Browser> {
    let settings = BrowserSettings {
        executable: crate::config::DEFAULT_BROWSER.into(),
        debug_port: 9333,
        profile_dir: std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-browser-profile"),
    };
    std::sync::Arc::new(Browser::new(settings).expect("тестовый браузер не создался"))
}

pub struct BrowserSettings {
    pub executable: PathBuf,
    pub debug_port: u16,
    pub profile_dir: PathBuf,
}

/// Браузер на базе Chromium (Edge, Chrome), которым программа управляет через отладочный порт.
pub struct Browser {
    settings: BrowserSettings,
    devtools: DevTools,
}

impl Browser {
    pub fn new(settings: BrowserSettings) -> Result<Self> {
        let devtools = DevTools::new(settings.debug_port)?;
        Ok(Self { settings, devtools })
    }

    /// Вкладка, адрес которой начинается с `home`. Закрытый браузер запускается, отсутствующая вкладка открывается.
    pub async fn tab(&self, home: &str) -> Result<Tab> {
        self.ensure_running(home).await?;
        match self.devtools.find_tab(home).await? {
            Some(tab) => Ok(tab),
            None => self.devtools.open_tab(home).await,
        }
    }

    async fn ensure_running(&self, start_url: &str) -> Result<()> {
        if self.devtools.is_available().await {
            return Ok(());
        }
        self.launch(start_url)?;
        if !wait::until(LAUNCH_TIMEOUT, || self.devtools.is_available()).await {
            bail!("браузер запустился, но порт {} не отвечает", self.settings.debug_port);
        }
        Ok(())
    }

    // Отдельный профиль обязателен: иначе уже открытый браузер перехватит запуск и порт не откроется.
    fn launch(&self, start_url: &str) -> Result<()> {
        Command::new(&self.settings.executable)
            .arg(format!("--remote-debugging-port={}", self.settings.debug_port))
            .arg(format!("--user-data-dir={}", self.settings.profile_dir.display()))
            .arg("--no-first-run")
            .arg(start_url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("не удалось запустить браузер {}", self.settings.executable.display()))?;
        Ok(())
    }
}
