use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Файлы JSON в папке `data`: слежки, находки, настройки.
#[derive(Debug, Clone)]
pub struct Storage {
    dir: PathBuf,
}

impl Storage {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Нет файла — значение по умолчанию. Испорченный файл не затирается молча: он откладывается рядом
    /// с пометкой `.broken`, чтобы его можно было восстановить руками.
    pub fn load<T: DeserializeOwned + Default>(&self, name: &str) -> T {
        let path = self.dir.join(name);
        let text = match fs::read_to_string(&path) {
            Ok(text) => text,
            Err(error) if error.kind() == ErrorKind::NotFound => return T::default(),
            Err(error) => {
                log::error!("не удалось прочитать {}: {error}", path.display());
                return T::default();
            }
        };
        serde_json::from_str(&text).unwrap_or_else(|error| {
            let broken = path.with_extension("json.broken");
            log::error!("{} испорчен ({error}), отложен в {}", path.display(), broken.display());
            if let Err(error) = fs::rename(&path, &broken) {
                log::error!("не удалось отложить испорченный файл: {error}");
            }
            T::default()
        })
    }

    /// Запись через временный файл: если программа упадёт посреди записи, старый файл останется целым.
    pub fn save<T: Serialize>(&self, name: &str, value: &T) -> Result<()> {
        fs::create_dir_all(&self.dir).with_context(|| format!("не удалось создать {}", self.dir.display()))?;
        let path = self.dir.join(name);
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, serde_json::to_string_pretty(value)?)
            .with_context(|| format!("не удалось записать {}", temporary.display()))?;
        fs::rename(&temporary, &path).with_context(|| format!("не удалось сохранить {}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn temporary_storage(test: &str) -> Storage {
        let dir = std::env::temp_dir().join(format!("ozon-price-bot-{test}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Storage::new(dir)
    }

    #[test]
    fn saved_value_is_loaded_back() {
        let storage = temporary_storage("roundtrip");
        let value = HashMap::from([("a".to_string(), 1u64)]);
        storage.save("value.json", &value).unwrap();
        assert_eq!(storage.load::<HashMap<String, u64>>("value.json"), value);
        let _ = fs::remove_dir_all(storage.dir());
    }

    #[test]
    fn missing_file_gives_default_and_broken_file_is_set_aside() {
        let storage = temporary_storage("broken");
        assert!(storage.load::<Vec<u64>>("absent.json").is_empty());

        fs::create_dir_all(storage.dir()).unwrap();
        fs::write(storage.dir().join("bad.json"), "{ not json").unwrap();
        assert!(storage.load::<Vec<u64>>("bad.json").is_empty());
        assert!(storage.dir().join("bad.json.broken").exists());
        let _ = fs::remove_dir_all(storage.dir());
    }
}
