use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;

use chrono::{Duration, Local, NaiveDate};

/// Столько дней хранятся файлы журнала: хватит разобрать сбой за прошлую неделю, а папка не растёт.
const KEPT_DAYS: i64 = 14;

/// Журнал работы и в консоль, и в файл: `logs\2026-10-29.log` в папке программы, новый файл каждый день.
/// Окно программы обычно свёрнуто, а слежки работают ночью — без файла причину ночного сбоя не узнать.
pub struct DailyLog {
    dir: PathBuf,
    day: Option<NaiveDate>,
    file: Option<File>,
}

impl DailyLog {
    pub fn new(dir: PathBuf) -> Self {
        Self {
            dir,
            day: None,
            file: None,
        }
    }

    /// Запись дня `day`: с новым днём — новый файл и уборка старых. Файл не открылся — журнал всё равно
    /// уходит в консоль, а программа работает дальше.
    fn write_on(&mut self, day: NaiveDate, text: &[u8]) {
        if self.day != Some(day) {
            self.day = Some(day);
            self.file = self.open(day).ok();
            self.remove_stale(day);
        }
        if let Some(file) = &mut self.file {
            let _ = file.write_all(text);
        }
    }

    fn open(&self, day: NaiveDate) -> io::Result<File> {
        fs::create_dir_all(&self.dir)?;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.dir.join(file_name(day)))
    }

    fn remove_stale(&self, today: NaiveDate) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            if entry.file_name().to_str().is_some_and(|name| is_stale(name, today)) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

impl Write for DailyLog {
    fn write(&mut self, text: &[u8]) -> io::Result<usize> {
        let _ = io::stderr().write_all(text);
        self.write_on(Local::now().date_naive(), text);
        Ok(text.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if let Some(file) = &mut self.file {
            file.flush()?;
        }
        io::stderr().flush()
    }
}

fn file_name(day: NaiveDate) -> String {
    format!("{}.log", day.format("%Y-%m-%d"))
}

/// Только файлы журнала по имени-дате: всё остальное в папке — не наше, его не трогаем.
fn is_stale(name: &str, today: NaiveDate) -> bool {
    name.strip_suffix(".log")
        .and_then(|date| NaiveDate::parse_from_str(date, "%Y-%m-%d").ok())
        .is_some_and(|day| day < today - Duration::days(KEPT_DAYS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, month, day).unwrap()
    }

    #[test]
    fn only_dated_logs_older_than_two_weeks_are_stale() {
        let today = day(10, 29);
        assert_eq!(file_name(today), "2026-10-29.log");
        assert!(!is_stale("2026-10-15.log", today), "ровно 14 дней — ещё храним");
        assert!(is_stale("2026-10-14.log", today));
        assert!(!is_stale("заметки.log", today));
        assert!(!is_stale("2026-10-01.txt", today));
    }

    #[test]
    fn new_day_starts_a_new_file_and_removes_old_ones() {
        let dir = std::env::temp_dir().join(format!("ozon-price-bot-logs-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("2026-10-01.log"), "старый").unwrap();
        fs::write(dir.join("заметки.txt"), "чужой файл").unwrap();
        let mut log = DailyLog::new(dir.clone());

        log.write_on(day(10, 28), b"first\n");
        log.write_on(day(10, 28), b"second\n");
        log.write_on(day(10, 29), b"third\n");
        drop(log);

        let read = |name: &str| fs::read_to_string(dir.join(name)).ok();
        let result = (
            read("2026-10-28.log"),
            read("2026-10-29.log"),
            read("2026-10-01.log"),
            read("заметки.txt"),
        );
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(
            result,
            (
                Some("first\nsecond\n".into()),
                Some("third\n".into()),
                None,
                Some("чужой файл".into())
            )
        );
    }
}
