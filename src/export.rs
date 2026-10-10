use crate::quantity::Unit;
use crate::search::Finding;

const HEADER: [&str; 8] = [
    "№",
    "Название",
    "Цена",
    "Старая цена",
    "Скидка %",
    "Рейтинг",
    "Отзывов",
    "Ссылка",
];
/// Колонка цены за единицу встаёт сразу за ценой — и только если сравнивали за единицу.
const UNIT_PRICE_COLUMN: usize = 3;

/// Таблица для русского Excel: метка UTF-8 в начале, точка с запятой между ячейками, CRLF между строками.
pub fn csv(findings: &[Finding], per_unit: Option<Unit>) -> Vec<u8> {
    let mut header: Vec<String> = HEADER.map(String::from).to_vec();
    if let Some(unit) = per_unit {
        header.insert(UNIT_PRICE_COLUMN, format!("Цена за {}", unit.label()));
    }
    let mut lines = vec![row(&header)];
    lines.extend(
        findings
            .iter()
            .enumerate()
            .map(|(index, found)| row(&cells(index + 1, found, per_unit.is_some()))),
    );
    format!("\u{feff}{}\r\n", lines.join("\r\n")).into_bytes()
}

fn cells(number: usize, found: &Finding, with_unit_price: bool) -> Vec<String> {
    let offer = &found.offer;
    let optional = |value: Option<u64>| value.map(|v| v.to_string()).unwrap_or_default();
    let mut cells = vec![
        number.to_string(),
        offer.name.clone(),
        offer.price.to_string(),
        optional(offer.old_price),
        optional(offer.discount_percent()),
        offer
            .rating
            .map(|rating| rating.to_string().replace('.', ","))
            .unwrap_or_default(),
        optional(offer.reviews),
        offer.url.clone(),
    ];
    if with_unit_price {
        // Числом с запятой, без «₽/кг»: так Excel сортирует колонку как числа.
        let rubles = found
            .unit_price
            .map(|price| format!("{},{:02}", price.kopecks / 100, price.kopecks % 100));
        cells.insert(UNIT_PRICE_COLUMN, rubles.unwrap_or_default());
    }
    cells
}

fn row(cells: &[String]) -> String {
    cells
        .iter()
        .map(|cell| format!("\"{}\"", cell.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(";")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::offer::Offer;
    use crate::quantity::UnitPrice;

    fn found(offer: Offer, unit_price: Option<UnitPrice>) -> Finding {
        Finding { offer, unit_price }
    }

    #[test]
    fn table_is_ready_for_russian_excel() {
        let offer = Offer {
            old_price: Some(7990),
            rating: Some(4.9),
            reviews: Some(8834),
            ..Offer::sample("Наушники \"Sony\"", 4364)
        };
        let text = String::from_utf8(csv(&[found(offer, None)], None)).unwrap();

        assert!(text.starts_with("\u{feff}\"№\";\"Название\";\"Цена\";\"Старая цена\""));
        assert!(text.ends_with("\r\n"));
        assert!(text.contains(
            "\"1\";\"Наушники \"\"Sony\"\"\";\"4364\";\"7990\";\"45\";\"4,9\";\"8834\";\"https://example.com/1\""
        ));
    }

    #[test]
    fn unit_price_column_follows_the_price() {
        let per_kilogram = UnitPrice {
            kopecks: 23_333,
            unit: Unit::Kilogram,
        };
        let rows = [
            found(Offer::sample("Порошок 6 кг", 1400), Some(per_kilogram)),
            found(Offer::sample("Порошок", 100), None),
        ];
        let text = String::from_utf8(csv(&rows, Some(Unit::Kilogram))).unwrap();

        assert!(text.contains("\"Цена\";\"Цена за кг\";\"Старая цена\""));
        assert!(text.contains("\"1\";\"Порошок 6 кг\";\"1400\";\"233,33\";\"\";"));
        assert!(text.contains("\"2\";\"Порошок\";\"100\";\"\";\"\";"));
    }
}
