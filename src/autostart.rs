use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// Запуск с этим словом открывает окно свёрнутым: при входе в Windows оно не должно заслонять рабочий стол.
pub const MINIMIZED_ARG: &str = "--minimized";

const LAUNCHER: &str = "Охота за выгодой.cmd";

/// Автозапуск при входе в Windows — файл в папке «Автозагрузка» пользователя. Планировщику заданий для запуска при входе
/// часто нужны права администратора, а службой программа быть не может: ей нужен рабочий стол (решение в `PLAN.md`).
pub struct Autostart {
    launcher: PathBuf,
}

impl Autostart {
    /// `None` — Windows не сказала, где папка пользователя: тогда галочки просто нет.
    pub fn of_this_user() -> Option<Self> {
        let app_data = env::var_os("APPDATA")?;
        Some(Self::in_folder(
            &Path::new(&app_data).join(r"Microsoft\Windows\Start Menu\Programs\Startup"),
        ))
    }

    fn in_folder(folder: &Path) -> Self {
        Self {
            launcher: folder.join(LAUNCHER),
        }
    }

    pub fn is_on(&self) -> bool {
        self.launcher.exists()
    }

    pub fn turn_on(&self, program: &Path) -> Result<()> {
        fs::write(&self.launcher, launcher_text(program))
            .with_context(|| format!("не удалось записать {}", self.launcher.display()))
    }

    pub fn turn_off(&self) -> Result<()> {
        match fs::remove_file(&self.launcher) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("не удалось удалить {}", self.launcher.display()))
            }
            _ => Ok(()),
        }
    }
}

/// `chcp 65001` — иначе кириллицу в пути к программе командная строка прочитает в старой кодировке.
fn launcher_text(program: &Path) -> String {
    format!(
        "@echo off\r\nchcp 65001 >nul\r\nstart \"\" \"{}\" {MINIMIZED_ARG}\r\n",
        program.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempFolder(PathBuf);

    impl TempFolder {
        fn new(test: &str) -> Self {
            let folder = env::temp_dir().join(format!("ozon-price-bot-autostart-{test}"));
            fs::create_dir_all(&folder).unwrap();
            Self(folder)
        }
    }

    impl Drop for TempFolder {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn turning_on_and_off_is_a_file_in_the_folder() {
        let folder = TempFolder::new("toggle");
        let autostart = Autostart::in_folder(&folder.0);
        assert!(!autostart.is_on());

        autostart
            .turn_on(Path::new(r"C:\Программы\ozon-price-bot.exe"))
            .unwrap();
        assert!(autostart.is_on());

        autostart.turn_off().unwrap();
        assert!(!autostart.is_on());
    }

    #[test]
    fn turning_off_what_is_not_on_is_not_a_failure() {
        let folder = TempFolder::new("off-twice");

        assert!(Autostart::in_folder(&folder.0).turn_off().is_ok());
    }

    #[test]
    fn launcher_starts_the_program_minimized_with_a_russian_path() {
        let text = launcher_text(Path::new(r"C:\Мои программы\ozon-price-bot.exe"));

        assert!(text.contains("chcp 65001"), "{text}");
        assert!(
            text.contains(r#"start "" "C:\Мои программы\ozon-price-bot.exe" --minimized"#),
            "{text}"
        );
    }
}
