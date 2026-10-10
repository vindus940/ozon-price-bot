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
    /// Когда привезут, как пишет площадка: «Завтра», «8 октября», «За час».
    #[serde(default)]
    pub delivery: Option<String>,
    /// На плитке Ozon стоит метка «Ozon»: товар продаёт сам маркетплейс, а не сторонний продавец.
    #[serde(default)]
    pub sold_by_ozon: bool,
    /// Отметка «Бренд проверен»: Ozon проверил, что продавец вправе торговать этим брендом.
    #[serde(default)]
    pub brand_verified: bool,
}

impl Offer {
    /// Привезут сегодня или завтра: «Сегодня», «Завтра», у экспресса — «За час», «За 2 часа».
    /// Неизвестный срок условие не проходит, как и неизвестный рейтинг.
    pub fn arrives_by_tomorrow(&self) -> bool {
        self.delivery.as_deref().is_some_and(|when| {
            let when = when.trim().to_lowercase();
            when == "сегодня" || when == "завтра" || when.starts_with("за ")
        })
    }

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
            delivery: None,
            sold_by_ozon: false,
            brand_verified: false,
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

    #[test]
    fn today_tomorrow_and_express_arrive_by_tomorrow() {
        let arriving = |when: Option<&str>| {
            Offer {
                delivery: when.map(Into::into),
                ..Offer::sample("x", 100)
            }
            .arrives_by_tomorrow()
        };
        for when in ["Сегодня", "Завтра", "За час", "За 2 часа"] {
            assert!(arriving(Some(when)), "{when}");
        }
        assert!(!arriving(Some("13 октября")));
        assert!(!arriving(Some("Послезавтра")));
        assert!(!arriving(None));
    }
}
