use std::fs::{self, File, TryLockError};
use std::io;
use std::path::Path;

const LOCK_FILE: &str = "program.lock";

/// Право работать одной программой на папку данных: два экземпляра водили бы одно окно браузера
/// (порт отладки один) и переписывали бы друг другу файлы слежек. Замок держит Windows, пока открыт файл,
/// и снимает сам, даже если программа упала. `None` — программа из этой папки уже запущена.
pub fn claim(data_dir: &Path) -> io::Result<Option<File>> {
    fs::create_dir_all(data_dir)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join(LOCK_FILE))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(file)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_launch_sees_the_first_until_it_exits() {
        let dir = std::env::temp_dir().join(format!("ozon-price-bot-instance-{}", std::process::id()));

        let first = claim(&dir).unwrap();
        assert!(first.is_some());
        assert!(
            claim(&dir).unwrap().is_none(),
            "пока первая работает, вторая не запускается"
        );

        drop(first);
        assert!(
            claim(&dir).unwrap().is_some(),
            "после выхода первой можно запуститься снова"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
