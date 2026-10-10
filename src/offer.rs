use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Offer {
    pub name: String,
    pub price: u64,
    pub old_price: Option<u64>,
    /// У Ozon — рейтинг товара, у Авито — рейтинг продавца.
    pub rating: Option<f32>,
    pub reviews: Option<u64>,
    pub url: String,
    /// Где лежит товар: «Валовая ул. Павелецкая». У Ozon нет — он везёт со склада.
    #[serde(default)]
    pub place: Option<String>,
    /// Расстояние от точки поиска, если площадка его сообщила.
    #[serde(default)]
    pub distance_km: Option<f64>,
    /// Когда опубликовано, как пишет площадка: «1 час назад».
    #[serde(default)]
    pub posted: Option<String>,
    /// Текст карточки целиком: в нём продавцы пишут «аналог», «не работает» и прочее, чего нет в заголовке.
    #[serde(default)]
    pub description: Option<String>,
}

impl Offer {
    pub fn discount_percent(&self) -> Option<u64> {
        self.old_price
            .filter(|&old| old > self.price)
            .map(|old| ((old - self.price) * 100 + old / 2) / old)
    }
}

#[cfg(test)]
impl Offer {
    pub fn sample(name: &str, price: u64) -> Self {
        Self {
            name: name.into(),
            price,
            old_price: None,
            rating: None,
            reviews: None,
            url: "https://example.com/1".into(),
            place: None,
            distance_km: None,
            posted: None,
            description: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discount_is_rounded_to_whole_percent() {
        let offer = Offer {
            old_price: Some(7990),
            ..Offer::sample("Sony WH-CH520", 4364)
        };
        assert_eq!(offer.discount_percent(), Some(45));
    }

    #[test]
    fn no_discount_without_higher_old_price() {
        assert_eq!(Offer::sample("x", 100).discount_percent(), None);
        assert_eq!(
            Offer {
                old_price: Some(100),
                ..Offer::sample("x", 100)
            }
            .discount_percent(),
            None
        );
    }
}
