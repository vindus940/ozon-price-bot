use crate::offer::Offer;

/// Сумма в рублях с разбивкой на тысячи: 72500 → «72 500 ₽» (с неразрывными пробелами).
pub fn rub(amount: u64) -> String {
    let digits = amount.to_string();
    let mut grouped = String::new();
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push('\u{a0}');
        }
        grouped.push(digit);
    }
    grouped + "\u{a0}₽"
}

/// Выгода может быть и отрицательной — тогда со знаком минус.
pub fn signed_rub(amount: i64) -> String {
    if amount < 0 {
        format!("−{}", rub(amount.unsigned_abs()))
    } else {
        rub(amount.unsigned_abs())
    }
}

/// Находка пришла снова, потому что подешевела: «5 000 ₽ → 4 500 ₽ (−10%)».
pub fn price_drop(before: u64, now: u64) -> String {
    let percent = before.saturating_sub(now) * 100 / before.max(1);
    format!("{} → {} (−{percent}%)", rub(before), rub(now))
}

/// Что ещё известно о предложении с плитки: «доставка завтра · продаёт Ozon · бренд проверен». Пусто, если ничего.
pub fn tile_marks(offer: &Offer) -> String {
    let delivery = offer
        .delivery
        .as_ref()
        .map(|when| format!("доставка {}", when.to_lowercase()));
    let seller = offer.sold_by_ozon.then(|| "продаёт Ozon".to_string());
    let brand = offer.brand_verified.then(|| "бренд проверен".to_string());
    [delivery, seller, brand]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rubles_are_grouped_by_thousands() {
        assert_eq!(rub(990), "990\u{a0}₽");
        assert_eq!(rub(1990), "1\u{a0}990\u{a0}₽");
        assert_eq!(rub(1_234_567), "1\u{a0}234\u{a0}567\u{a0}₽");
        assert_eq!(signed_rub(-1500), "−1\u{a0}500\u{a0}₽");
    }

    #[test]
    fn price_drop_names_both_prices_and_the_percent() {
        assert_eq!(price_drop(5_000, 4_500), "5\u{a0}000\u{a0}₽ → 4\u{a0}500\u{a0}₽ (−10%)");
    }

    #[test]
    fn tile_marks_read_as_one_line_and_skip_the_unknown() {
        let ozon = Offer {
            delivery: Some("Завтра".into()),
            sold_by_ozon: true,
            brand_verified: true,
            ..Offer::sample("Наушники", 3_870)
        };
        assert_eq!(tile_marks(&ozon), "доставка завтра · продаёт Ozon · бренд проверен");
        assert_eq!(tile_marks(&Offer::sample("Шуруповерт с Авито", 3_000)), "");
    }
}
