use std::collections::BTreeSet;

use crate::text::words;

/// Цвет и общие слова карточки: от них модель не меняется. Русских цветов здесь нет: кириллица в отпечаток не идёт.
const SAME_MODEL_WORDS: &[&str] = &[
    "black",
    "white",
    "blue",
    "red",
    "green",
    "gray",
    "grey",
    "pink",
    "silver",
    "gold",
    "beige",
    "purple",
    "yellow",
    "orange",
    "brown",
    "midnight",
    "usb",
    "type",
    "c",
    "bluetooth",
    "wireless",
    "tws",
    "hi",
    "res",
    "audio",
];

/// Русские слова, которые меняют товар: «iPhone 15 Про» — не «iPhone 15», а уценённый — не новый.
const OTHER_MODEL_WORDS: &[&str] = &[
    "про",
    "макс",
    "плюс",
    "мини",
    "ультра",
    "лайт",
    "комплект",
    "набор",
    "уцененный",
    "уцененные",
    "уценка",
];

/// Отпечаток модели по названию: у одной модели разных цветов и продавцов он совпадает.
/// Лучше не склеить одну модель, чем склеить разные, поэтому отпечатка нет, когда уверенности нет: без слова вроде
/// «ch520» или «128gb» и у вещей «для» модели, которые называют её целиком.
pub(super) fn model_key(title: &str) -> Option<BTreeSet<String>> {
    let title_words = words(title);
    if title_words.iter().any(|word| word == "для") {
        return None;
    }
    let key: BTreeSet<String> = title_words.into_iter().filter(|word| tells_model(word)).collect();
    key.iter().any(|word| is_model_number(word)).then_some(key)
}

fn tells_model(word: &str) -> bool {
    let latin_or_number = word.chars().all(|c| c.is_ascii_alphanumeric()) || word.chars().any(char::is_numeric);
    (latin_or_number && !SAME_MODEL_WORDS.contains(&word)) || OTHER_MODEL_WORDS.contains(&word)
}

fn is_model_number(word: &str) -> bool {
    word.chars().any(char::is_numeric) && word.chars().any(char::is_alphabetic)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn same(first: &str, second: &str) -> bool {
        model_key(first).is_some() && model_key(first) == model_key(second)
    }

    #[test]
    fn colors_order_of_words_and_card_words_do_not_change_the_model() {
        assert!(same(
            "Наушники Sony WH-CH520, черные",
            "SONY Беспроводные наушники WH-CH520 синие"
        ));
        assert!(same(
            "Наушники Sony WH-CH520 Black",
            "Sony WH-CH520 Bluetooth, USB Type-C, белый"
        ));
    }

    #[test]
    fn other_version_memory_or_quantity_is_another_model() {
        assert!(!same("Apple iPhone 15 128GB", "Apple iPhone 15 Pro 128GB"));
        assert!(!same("Apple iPhone 15 128GB", "Apple iPhone 15 Про 128GB"));
        assert!(!same("Apple iPhone 15 128GB", "Apple iPhone 15 256GB"));
        assert!(!same("Смартфон X100 128гб", "Смартфон X100 256гб"));
        assert!(!same("Sony WH-CH520", "Sony WH-CH720"));
        assert!(!same("Наушники SONY WI-C100", "Наушники SONY WI-C100. Товар уценённый"));
        assert!(!same("Батарейки AA2 2 шт", "Батарейки AA2 4 шт"));
    }

    #[test]
    fn without_a_model_number_nothing_is_certain() {
        assert_eq!(model_key("Наушники Sony"), None);
        assert_eq!(model_key("Чайник Bosch 2000 Вт"), None);
    }

    #[test]
    fn thing_for_a_model_is_not_that_model() {
        assert_eq!(model_key("Амбушюры для наушников Sony WH-CH520"), None);
    }
}
