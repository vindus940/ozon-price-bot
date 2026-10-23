mod devtools;

use std::env;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::wait;
use devtools::DevTools;
pub use devtools::Tab;

const LAUNCH_TIMEOUT: Duration = Duration::from_secs(15);
const EDGE: &str = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
/// Куда Chrome ставит установщик: для всех пользователей или только для текущего.
const CHROME_HOMES: [&str; 3] = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"];

/// Chrome, если он установлен, иначе Edge — он есть в каждой Windows.
pub fn installed_browser() -> PathBuf {
    CHROME_HOMES
        .iter()
        .filter_map(env::var_os)
        .map(|home| PathBuf::from(home).join(r"Google\Chrome\Application\chrome.exe"))
        .find(|chrome| chrome.exists())
        .unwrap_or_else(|| PathBuf::from(EDGE))
}

/// Папка профиля — своя у каждого браузера: Chrome не открывает профиль Edge. У Edge она называется, как раньше,
/// `browser-profile`: в ней пройденные проверки «я не робот» и выбранный город ДНС.
pub fn profile_name(executable: &Path) -> String {
    let browser = executable
        .file_stem()
        .map_or_else(String::new, |stem| stem.to_string_lossy().to_lowercase());
    match browser.as_str() {
        "msedge" => "browser-profile".into(),
        // Яндекс.Браузер — это browser.exe: по общему правилу его профиль совпал бы с профилем Edge.
        "browser" => "yandex-profile".into(),
        other => format!("{other}-profile"),
    }
}

/// Отдельный браузер для живых тестов: свой порт и профиль в `target`, чтобы не трогать окно программы.
#[cfg(test)]
pub fn test_browser() -> std::sync::Arc<Browser> {
    let executable = installed_browser();
    let settings = BrowserSettings {
        profile_dir: Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join(format!("test-{}", profile_name(&executable))),
        executable,
        debug_port: 9333,
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

    /// Вкладка, адрес которой начинается с `home`, — разбуженная. Закрытый браузер запускается, отсутствующая вкладка
    /// открывается.
    pub async fn tab(&self, home: &str) -> Result<Tab> {
        self.ensure_running(home).await?;
        let tab = match self.devtools.find_tab(home).await? {
            Some(tab) => tab,
            None => self.devtools.open_tab(home).await?,
        };
        // Не разбудилась — всё равно пробуем: в незамороженной вкладке поиск пройдёт и так.
        if let Err(error) = tab.wake().await {
            log::warn!("{error:#}");
        }
        Ok(tab)
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
            .arg("--no-default-browser-check")
            // Окно браузера программы обычно свёрнуто или закрыто другими окнами, а в скрытых вкладках браузер растягивает
            // таймеры — через пять минут до раза в минуту, а паузы между порциями Ozon стоят на таймерах.
            .args([
                "--disable-background-timer-throttling",
                "--disable-renderer-backgrounding",
                "--disable-backgrounding-occluded-windows",
            ])
            .arg(start_url)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("не удалось запустить браузер {}", self.settings.executable.display()))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_browser_has_its_own_profile_and_edge_keeps_the_old_one() {
        let name = |path: &str| profile_name(Path::new(path));
        assert_eq!(
            name(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            "chrome-profile"
        );
        assert_eq!(
            name(EDGE),
            "browser-profile",
            "пройденные проверки и город ДНС остаются у Edge"
        );
        assert_eq!(
            name(r"C:\Users\me\AppData\Local\Yandex\YandexBrowser\Application\browser.exe"),
            "yandex-profile"
        );
    }
}
