use serde::{Deserialize, Serialize};

use crate::hunt::watch::Watch;

pub const FILE: &str = "watches.json";

/// Список слежек и счётчик номеров. Номер не переиспользуется: память об отправленном и находки
/// удалённой слежки не достанутся новой.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct WatchBook {
    next_id: u64,
    watches: Vec<Watch>,
}

impl WatchBook {
    pub fn all(&self) -> &[Watch] {
        &self.watches
    }

    pub fn get(&self, watch_id: u64) -> Option<&Watch> {
        self.watches.iter().find(|watch| watch.id == watch_id)
    }

    /// Новая слежка (id = 0) получает номер, существующая заменяется. Ошибка — её уже удалили,
    /// пока была открыта форма.
    pub fn save(&mut self, mut watch: Watch) -> Result<u64, String> {
        if watch.id == 0 {
            self.next_id += 1;
            watch.id = self.next_id;
            self.watches.push(watch);
            return Ok(self.next_id);
        }
        let existing = self
            .watches
            .iter_mut()
            .find(|existing| existing.id == watch.id)
            .ok_or_else(|| "Эту слежку уже удалили — создайте новую".to_string())?;
        *existing = watch;
        Ok(existing.id)
    }

    pub fn remove(&mut self, watch_id: u64) -> Option<Watch> {
        let index = self.watches.iter().position(|watch| watch.id == watch_id)?;
        Some(self.watches.remove(index))
    }

    pub fn set_enabled(&mut self, watch_id: u64, enabled: bool) -> bool {
        let Some(watch) = self.watches.iter_mut().find(|watch| watch.id == watch_id) else {
            return false;
        };
        watch.enabled = enabled;
        true
    }

    /// Слежка та же, что в начале проверки: её не изменили и не удалили, пока шёл поиск.
    /// Включение и выключение не в счёт — найденное от этого не устаревает.
    pub fn still_has(&self, checked: &Watch) -> bool {
        self.get(checked.id).is_some_and(|current| {
            Watch {
                enabled: checked.enabled,
                ..current.clone()
            } == *checked
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch(name: &str) -> Watch {
        Watch {
            name: name.into(),
            query: "iphone 15".into(),
            ..Watch::new_draft()
        }
    }

    #[test]
    fn numbers_are_never_reused() {
        let mut book = WatchBook::default();
        let first = book.save(watch("первая")).unwrap();
        book.remove(first);
        let second = book.save(watch("вторая")).unwrap();
        assert_eq!((first, second), (1, 2));
    }

    fn renamed(book: &WatchBook, watch_id: u64, name: &str) -> Watch {
        let mut edited = book.get(watch_id).unwrap().clone();
        edited.name = name.into();
        edited
    }

    #[test]
    fn edit_replaces_but_deleted_watch_cannot_be_saved() {
        let mut book = WatchBook::default();
        let id = book.save(watch("айфон")).unwrap();

        book.save(renamed(&book, id, "айфон 15")).unwrap();
        assert_eq!(book.get(id).unwrap().name, "айфон 15");

        let edited_in_open_form = renamed(&book, id, "айфон 15 pro");
        book.remove(id);
        let problem = book.save(edited_in_open_form).unwrap_err();
        assert!(problem.contains("уже удалили"));
        assert!(book.all().is_empty());
    }

    #[test]
    fn toggling_does_not_make_a_running_check_stale_but_editing_does() {
        let mut book = WatchBook::default();
        let id = book.save(watch("айфон")).unwrap();
        let checked = book.get(id).unwrap().clone();

        book.set_enabled(id, false);
        assert!(book.still_has(&checked));

        book.save(renamed(&book, id, "айфон 15")).unwrap();
        assert!(!book.still_has(&checked));
    }
}
