use std::collections::VecDeque;

use chrono::{DateTime, Local};

const KEPT: usize = 300;

#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub at: DateTime<Local>,
    pub text: String,
}

/// Что охотник делал — для вкладки «Журнал» и лога. Живёт только в памяти: после перезапуска начинается заново.
#[derive(Debug, Default)]
pub struct Journal {
    entries: VecDeque<JournalEntry>,
}

impl Journal {
    pub fn write(&mut self, text: String) {
        log::info!("{text}");
        self.entries.push_front(JournalEntry { at: Local::now(), text });
        self.entries.truncate(KEPT);
    }

    /// Свежие первыми.
    pub fn entries(&self) -> Vec<JournalEntry> {
        self.entries.iter().cloned().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_latest_entries_newest_first() {
        let mut journal = Journal::default();
        for number in 1..=KEPT + 5 {
            journal.write(number.to_string());
        }

        let entries = journal.entries();

        assert_eq!(entries.len(), KEPT);
        assert_eq!(entries[0].text, (KEPT + 5).to_string());
        assert_eq!(entries[KEPT - 1].text, "6");
    }
}
