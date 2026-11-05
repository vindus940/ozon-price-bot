use anyhow::Result;
use rust_xlsxwriter::{Format, Workbook, Worksheet};

use crate::quantity::Unit;
use crate::search::Finding;

const HEADER: [&str; 13] = [
    "№",
    "Название",
    "Цена",
    "Старая цена",
    "Скидка %",
    "Рейтинг",
    "Отзывов",
    "Доставка",
    "Продаёт Ozon",
    "Бренд проверен",
    "Намного дешевле обычного",
    "Площадка",
    "Ссылка",
];
/// Колонка цены за единицу встаёт сразу за ценой — и только если сравнивали за единицу.
const UNIT_PRICE_COLUMN: usize = 3;
const NAME_COLUMN: u16 = 1;
const NAME_WIDTH: f64 = 60.0;
const USUAL_WIDTH: f64 = 13.0;

#[derive(Debug, PartialEq)]
enum Cell {
    Empty,
    Text(String),
    /// Числом, а не текстом: так Excel сортирует и фильтрует колонку как числа.
    Number(f64),
    Link(String),
}

/// Таблица Excel: шапка закреплена, по колонкам есть автофильтр, числа — числами, ссылки — ссылками.
pub fn xlsx(findings: &[Finding], per_unit: Option<Unit>) -> Result<Vec<u8>> {
    let header = header(per_unit);
    let mut workbook = Workbook::new();
    let sheet = workbook.add_worksheet();
    let bold = Format::new().set_bold();
    for (column, title) in (0u16..).zip(&header) {
        sheet.write_string_with_format(0, column, title, &bold)?;
        sheet.set_column_width(column, USUAL_WIDTH)?;
    }
    sheet.set_column_width(NAME_COLUMN, NAME_WIDTH)?;
    for (row, found) in (1u32..).zip(findings) {
        for (column, cell) in (0u16..).zip(cells(row, found, per_unit.is_some())) {
            write(sheet, row, column, &cell)?;
        }
    }
    let last_column = u16::try_from(header.len() - 1)?;
    sheet.set_freeze_panes(1, 0)?;
    sheet.autofilter(0, 0, u32::try_from(findings.len())?, last_column)?;
    Ok(workbook.save_to_buffer()?)
}

fn write(sheet: &mut Worksheet, row: u32, column: u16, cell: &Cell) -> Result<()> {
    match cell {
        Cell::Empty => {}
        Cell::Text(text) => {
            sheet.write_string(row, column, text)?;
        }
        Cell::Number(number) => {
            sheet.write_number(row, column, *number)?;
        }
        // Слишком длинную ссылку Excel не принимает: тогда она остаётся текстом, а таблица всё равно приходит.
        Cell::Link(link) => {
            if sheet.write_url(row, column, link.as_str()).is_err() {
                sheet.write_string(row, column, link)?;
            }
        }
    }
    Ok(())
}

fn header(per_unit: Option<Unit>) -> Vec<String> {
    let mut header: Vec<String> = HEADER.map(String::from).to_vec();
    if let Some(unit) = per_unit {
        header.insert(UNIT_PRICE_COLUMN, format!("Цена за {}", unit.label()));
    }
    header
}

#[allow(clippy::cast_precision_loss)]
fn number(value: u64) -> Cell {
    Cell::Number(value as f64)
}

fn cells(number_in_list: u32, found: &Finding, with_unit_price: bool) -> Vec<Cell> {
    let offer = &found.offer;
    let optional = |value: Option<u64>| value.map_or(Cell::Empty, number);
    let mut cells = vec![
        Cell::Number(f64::from(number_in_list)),
        Cell::Text(offer.name.clone()),
        number(offer.price),
        optional(offer.old_price),
        optional(offer.discount_percent()),
        offer.rating.map_or(Cell::Empty, |rating| {
            Cell::Number((f64::from(rating) * 100.0).round() / 100.0)
        }),
        optional(offer.reviews),
        offer.delivery.clone().map_or(Cell::Empty, Cell::Text),
        yes(offer.sold_by_ozon),
        yes(offer.brand_verified),
        yes(found.suspicious),
        Cell::Text(found.source.title().to_string()),
        Cell::Link(offer.url.clone()),
    ];
    if with_unit_price {
        #[allow(clippy::cast_precision_loss)]
        let rubles = found
            .unit_price
            .map_or(Cell::Empty, |price| Cell::Number(price.kopecks as f64 / 100.0));
        cells.insert(UNIT_PRICE_COLUMN, rubles);
    }
    cells
}

/// «да» или пусто: так в Excel фильтр по колонке показывает ровно два значения.
fn yes(flag: bool) -> Cell {
    if flag { Cell::Text("да".into()) } else { Cell::Empty }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::marketplace::SourceKind;
    use crate::offer::Offer;
    use crate::quantity::UnitPrice;

    fn found(offer: Offer, unit_price: Option<UnitPrice>) -> Finding {
        Finding {
            source: SourceKind::Ozon,
            offer,
            unit_price,
            suspicious: false,
        }
    }

    #[test]
    fn numbers_are_numbers_links_are_links_and_marks_are_yes_or_empty() {
        let offer = Offer {
            old_price: Some(7990),
            rating: Some(4.9),
            reviews: Some(8834),
            delivery: Some("Завтра".into()),
            sold_by_ozon: true,
            ..Offer::sample("Наушники \"Sony\"", 4364)
        };
        let suspicious = Finding {
            suspicious: true,
            ..found(offer, None)
        };

        assert_eq!(
            cells(1, &suspicious, false),
            [
                Cell::Number(1.0),
                Cell::Text("Наушники \"Sony\"".into()),
                Cell::Number(4364.0),
                Cell::Number(7990.0),
                Cell::Number(45.0),
                Cell::Number(4.9),
                Cell::Number(8834.0),
                Cell::Text("Завтра".into()),
                Cell::Text("да".into()),
                Cell::Empty,
                Cell::Text("да".into()),
                Cell::Text("Ozon".into()),
                Cell::Link("https://example.com/1".into()),
            ]
        );
    }

    #[test]
    fn unit_price_column_follows_the_price() {
        let per_kilogram = UnitPrice {
            kopecks: 23_333,
            unit: Unit::Kilogram,
        };

        assert_eq!(header(Some(Unit::Kilogram))[2..4], ["Цена", "Цена за кг"]);
        let measured = cells(1, &found(Offer::sample("Порошок 6 кг", 1400), Some(per_kilogram)), true);
        assert_eq!(measured[3], Cell::Number(233.33));
        let unmeasured = cells(2, &found(Offer::sample("Порошок", 100), None), true);
        assert_eq!(unmeasured[3], Cell::Empty);
        assert_eq!(unmeasured.len(), header(Some(Unit::Kilogram)).len());
    }

    #[test]
    fn every_row_fits_the_header() {
        assert_eq!(
            cells(1, &found(Offer::sample("Порошок", 100), None), false).len(),
            header(None).len()
        );
    }

    #[test]
    fn workbook_is_written_even_with_a_link_excel_would_not_take() {
        let long = Offer {
            url: format!("https://example.com/{}", "a".repeat(3_000)),
            ..Offer::sample("Наушники", 1000)
        };

        let bytes = xlsx(&[found(long, None)], None).unwrap();

        assert!(bytes.starts_with(b"PK"), "файл xlsx — это архив zip");
    }
}
