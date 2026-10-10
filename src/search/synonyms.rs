/// Как пишут один и тот же бренд: латиницей и по-русски. Продавцы на Авито часто пишут «Макита» вместо «Makita»,
/// и без словаря запрос «makita» таких объявлений не находит. Записи — строчными и без «ё», как после `normalize`.
/// Подобраны под категории программы: электроника, бытовая техника, инструменты.
const BRANDS: &[&[&str]] = &[
    // Телефоны и электроника
    &["apple", "эпл", "эппл"],
    &["iphone", "айфон"],
    &["ipad", "айпад"],
    &["macbook", "макбук"],
    &["airpods", "эйрподс", "аирподс"],
    &["samsung", "самсунг"],
    &["galaxy", "галакси", "гэлакси"],
    &["xiaomi", "сяоми", "ксиаоми", "ксиоми"],
    &["redmi", "редми"],
    &["huawei", "хуавей", "хуавэй"],
    &["honor", "хонор"],
    &["realme", "реалми"],
    &["google", "гугл"],
    &["pixel", "пиксель"],
    &["sony", "сони"],
    &["playstation", "плейстейшн", "плейстейшен"],
    &["xbox", "иксбокс"],
    &["nintendo", "нинтендо"],
    &["marshall", "маршал", "маршалл"],
    &["lenovo", "леново"],
    &["asus", "асус"],
    &["acer", "асер"],
    &["canon", "кэнон", "канон"],
    &["nikon", "никон"],
    &["gopro", "гопро"],
    // Бытовая техника
    &["dyson", "дайсон"],
    &["bosch", "бош"],
    &["philips", "филипс"],
    &["braun", "браун"],
    &["tefal", "тефаль"],
    &["delonghi", "делонги"],
    &["karcher", "керхер"],
    &["electrolux", "электролюкс"],
    &["miele", "миле"],
    &["siemens", "сименс"],
    &["indesit", "индезит"],
    &["haier", "хайер", "хаер"],
    &["redmond", "редмонд"],
    &["polaris", "поларис"],
    &["kitfort", "китфорт"],
    &["nespresso", "неспрессо"],
    &["gorenje", "горенье"],
    &["atlant", "атлант"],
    // Инструменты
    &["makita", "макита"],
    &["dewalt", "девольт", "деволт", "дэволт"],
    &["metabo", "метабо"],
    &["hitachi", "хитачи"],
    &["hikoki", "хикоки"],
    &["ryobi", "риоби", "реби"],
    &["milwaukee", "милуоки", "милвоки"],
    &["einhell", "айнхель"],
    &["stihl", "штиль"],
    &["husqvarna", "хускварна"],
    &["interskol", "интерскол"],
    &["zubr", "зубр"],
    &["hilti", "хилти"],
    &["festool", "фестул"],
    &["worx", "воркс"],
    &["greenworks", "гринворкс"],
];

/// Все написания слова: для бренда из словаря — латиница и кириллица, для остальных слов — само слово.
pub fn spellings(word: &str) -> Vec<&str> {
    BRANDS
        .iter()
        .find(|brand| brand.contains(&word))
        .map_or_else(|| vec![word], |brand| brand.to_vec())
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::text::normalize;

    #[test]
    fn brand_is_found_by_any_spelling() {
        assert_eq!(spellings("makita"), ["makita", "макита"]);
        assert_eq!(spellings("макита"), ["makita", "макита"]);
        assert_eq!(spellings("бош"), ["bosch", "бош"]);
        assert_eq!(spellings("шуруповерт"), ["шуруповерт"]);
    }

    #[test]
    fn every_spelling_belongs_to_one_brand_and_is_normalized() {
        let mut seen = HashSet::new();
        for spelling in BRANDS.iter().flat_map(|brand| brand.iter()) {
            assert!(seen.insert(*spelling), "«{spelling}» встречается дважды");
            assert_eq!(
                normalize(spelling),
                *spelling,
                "«{spelling}» записан не строчными или с «ё»"
            );
        }
    }
}
