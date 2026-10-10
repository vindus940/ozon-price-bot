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
}
