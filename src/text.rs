/// Нижний регистр и «ё» → «е»: так «Ёлка» и «елка» считаются одним словом.
pub fn normalize(text: &str) -> String {
    text.to_lowercase().replace('ё', "е")
}

/// Отдельные слова текста, без знаков препинания: «iPhone 15 Pro, 128GB» → iphone, 15, pro, 128gb.
pub fn words(text: &str) -> Vec<String> {
    normalize(text)
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_are_normalized_and_split_on_punctuation() {
        assert_eq!(
            words("Apple iPhone 15 Pro, 128GB (Ёмкий)"),
            ["apple", "iphone", "15", "pro", "128gb", "емкий"]
        );
    }
}
