use crate::hunt::category::Category;
use crate::hunt::watch::{Goal, Watch};
use crate::offer::Offer;

/// Общее для тестов охотника: рынок айфонов и слежка за ним.
pub fn phone(price: u64) -> Offer {
    Offer {
        url: format!("https://example.com/{price}"),
        ..Offer::sample("Apple iPhone 15 128GB", price)
    }
}

/// Шесть айфонов по 70–75 тысяч, чехол и версия Pro: рынок — 72 500 ₽ по шести похожим.
pub fn market_offers() -> Vec<Offer> {
    let mut offers: Vec<Offer> = [70_000, 71_000, 72_000, 73_000, 74_000, 75_000].map(phone).to_vec();
    offers.push(Offer {
        url: "https://example.com/case".into(),
        ..Offer::sample("Чехол для iPhone 15 128GB", 900)
    });
    offers.push(Offer {
        url: "https://example.com/pro".into(),
        ..Offer::sample("Apple iPhone 15 Pro 128GB", 95_000)
    });
    offers
}

pub fn iphone_watch() -> Watch {
    Watch {
        name: "Айфон".into(),
        query: "iphone 15 128".into(),
        category: Category::Electronics,
        goal: Goal::BelowMarket {
            min_discount_percent: 15,
        },
        ..Watch::new_draft()
    }
}
