use crate::format::{rub, signed_rub, tile_marks};
use crate::health::{Health, Problem};
use crate::hunt::{Alert, Deal, Goal, Watch};
use crate::marketplace::SourceError;
use crate::offer::Offer;
use crate::search::{Finding, Findings, Order, PriceRange, SearchRequest};

const MESSAGE_LIMIT: usize = 4096;
const NAME_LIMIT: usize = 90;
const SAMPLE_NAME_LIMIT: usize = 60;

pub(super) const HELP: &str = "Напишите, что ищете, — пришлю самые дешёвые предложения и таблицу для Excel.

Цену можно ограничить словами <b>от</b> и <b>до</b>:
<code>наушники sony до 5000</code>
<code>iphone 15 от 50 000 до 80 000</code>

Лишнее убирается минус-словами:
<code>наушники sony -амбушюры -чехол</code>

Можно отсечь товары с низким рейтингом и без отзывов:
<code>наушники sony рейтинг от 4.5 отзывов от 100</code>

Только то, что привезут не позже завтра, и только от самого Ozon:
<code>наушники sony доставка завтра только ozon</code>

Продукты и химию честнее сравнивать по цене за килограмм, литр или штуку — пачка 400 г не будет «дешевле» мешка на 3 кг:
<code>стиральный порошок за кг</code>
<code>капсулы для стирки за шт</code>

Сначала идут самые дешёвые, но можно иначе:
<code>наушники sony по скидке</code> — сначала с самой большой скидкой
<code>наушники sony цена-качество</code> — и недорого, и хорошо оценено многими покупателями";

pub(super) fn access_denied(user_id: u64) -> String {
    format!("Доступ закрыт. Ваш Telegram ID: {user_id}. Впишите его в ALLOWED_USERS в файле .env и перезапустите бота.")
}

pub(super) fn searching(query: &str, source: &str) -> String {
    format!("Ищу «{query}» на {source}…")
}

pub(super) fn findings(request: &SearchRequest, findings: &Findings, top: usize) -> String {
    let query = escape(&request.query);
    let conditions = conditions(request);
    if findings.suitable.is_empty() {
        let hint = if conditions.is_empty() {
            "Попробуйте убрать из запроса лишние слова — в названии товара должны быть все."
        } else {
            "Попробуйте убрать из запроса лишние слова или ослабить условия."
        };
        return format!(
            "По запросу «{query}» подходящих предложений нет (просмотрено: {}).\n{hint}",
            findings.scanned
        );
    }

    let mut text = format!(
        "<b>{query}</b>{conditions}\nПодходят {} из {}, {}:\n",
        findings.suitable.len(),
        findings.scanned,
        order(request, findings)
    );
    let hint = unit_price_hint(request, findings);
    for (index, found) in findings.suitable.iter().take(top).enumerate() {
        let entry = offer_entry(index + 1, found);
        if text.chars().count() + entry.chars().count() + hint.chars().count() > MESSAGE_LIMIT {
            break;
        }
        text += &entry;
    }
    text + &hint
}

/// Как упорядочен ответ — с оговоркой, если просьбу выполнить не из чего: нет фасовки или нет скидок.
fn order(request: &SearchRequest, findings: &Findings) -> String {
    let measured = findings.suitable.iter().any(|found| found.unit_price.is_some());
    let discounted = findings
        .suitable
        .iter()
        .any(|found| found.offer.discount_percent().is_some());
    let per_unit = request
        .per_unit
        .filter(|_| measured)
        .map(|unit| format!(" за {}", unit.label()))
        .unwrap_or_default();

    let mut caveats = Vec::new();
    if let Some(unit) = request.per_unit.filter(|_| !measured) {
        caveats.push(format!("фасовки в {} в названиях нет", unit.label()));
    }
    let order = match request.order {
        Order::Cheapest => format!("самые дешёвые{per_unit}"),
        Order::BiggestDiscount if discounted => "с самой большой скидкой".into(),
        Order::BiggestDiscount => {
            caveats.push("скидок нет ни у одного".into());
            format!("самые дешёвые{per_unit}")
        }
        Order::PriceAndRating => format!("лучшие по цене{per_unit} и рейтингу"),
    };
    if caveats.is_empty() {
        order
    } else {
        format!("{order} ({})", caveats.join(", "))
    }
}

/// У большинства найденного фасовка в названии, а сравнивали за пачку — подсказать, как сравнить честно.
/// Порядок из запроса подсказка сохраняет: «по скидке» не должно теряться при переходе к цене за килограмм.
fn unit_price_hint(request: &SearchRequest, findings: &Findings) -> String {
    let order = match request.order {
        Order::Cheapest => "",
        Order::BiggestDiscount => " по скидке",
        Order::PriceAndRating => " цена-качество",
    };
    match (request.per_unit, findings.packed_in) {
        (None, Some(unit)) => format!(
            "\nСравнить по цене за {unit}: <code>{} за {unit}{order}</code>",
            escape(&request.query),
            unit = unit.label()
        ),
        _ => String::new(),
    }
}

pub(super) fn failure(error: &SourceError, source: &str) -> String {
    match error {
        SourceError::Captcha => format!(
            "{source} просит подтвердить, что вы не робот. Откройте окно браузера с {source}, пройдите проверку и повторите запрос."
        ),
        SourceError::NotReady => format!(
            "{source} не открылся. Если в окне браузера проверка «я не робот» — пройдите её и повторите запрос."
        ),
        SourceError::Failed(error) => format!("Не получилось: {error:#}"),
    }
}

pub(super) fn health(health: &Health, source: &str, query: &str) -> String {
    match health {
        Health::Working { offers } => format!(
            "Самопроверка {source}: снова всё в порядке — по контрольному запросу «{query}» найдено товаров: {offers}."
        ),
        Health::Broken { problems, sample } => {
            let problems = problems.iter().map(problem).collect::<Vec<_>>().join("\n— ");
            let sample = if sample.is_empty() {
                "ни одного товара".to_string()
            } else {
                sample
                    .iter()
                    .enumerate()
                    .map(|(index, offer)| sample_line(index + 1, offer))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!(
                "Самопроверка {source}: похоже, сломался разбор выдачи — скорее всего, {source} поменял устройство страницы.\n\n\
                 Контрольный запрос «{query}»:\n— {problems}\n\n\
                 Что пришло:\n{sample}\n\n\
                 Пока скрипт разбора этой площадки не поправлен, бот может находить не всё."
            )
        }
        Health::Unavailable(error) => format!("Самопроверка {source} не прошла. {}", failure(error, source)),
    }
}

fn problem(problem: &Problem) -> String {
    match problem {
        Problem::TooFewOffers { found, expected } => {
            format!("найдено товаров: {found}, а должно быть не меньше {expected}")
        }
        Problem::MissingNames { count } => format!("товаров без названия: {count}"),
        Problem::NoRatings => "ни у одного товара нет рейтинга".into(),
        Problem::NoReviews => "ни у одного товара нет числа отзывов".into(),
    }
}

fn sample_line(place: usize, offer: &Offer) -> String {
    let name = if offer.name.is_empty() {
        "(без названия)".to_string()
    } else {
        shorten(&offer.name, SAMPLE_NAME_LIMIT)
    };
    format!("{place}. {} — {name} — {}", rub(offer.price), offer.url)
}

pub(super) fn table_file_name(query: &str, source: &str) -> String {
    let slug: String = query
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '_' })
        .take(40)
        .collect();
    format!("{}_{slug}.csv", source.to_lowercase())
}

fn offer_entry(place: usize, found: &Finding) -> String {
    let offer = &found.offer;
    let unit_price = found.unit_price.map(|price| format!(" · {price}")).unwrap_or_default();
    let discount = match (offer.old_price, offer.discount_percent()) {
        (Some(old), Some(percent)) => format!(" <s>{}</s> −{percent}%", rub(old)),
        _ => String::new(),
    };
    let rating = match (offer.rating, offer.reviews) {
        (Some(rating), Some(reviews)) => format!("\n★ {rating} · отзывов: {reviews}"),
        (Some(rating), None) => format!("\n★ {rating}"),
        _ => String::new(),
    };
    let marks = marks_line(offer);
    format!(
        "\n{place}. <b>{price}</b>{unit_price}{discount}\n<a href=\"{url}\">{name}</a>{rating}{marks}\n",
        price = rub(offer.price),
        url = escape(&offer.url),
        name = escape(&shorten(&offer.name, NAME_LIMIT)),
    )
}

fn conditions(request: &SearchRequest) -> String {
    let conditions: Vec<String> = [
        price_condition(request.price),
        request.min_rating.map(|rating| format!("рейтинг от {rating}")),
        request.min_reviews.map(|reviews| format!("отзывов от {reviews}")),
    ]
    .into_iter()
    .flatten()
    .chain(request.ozon.labels().into_iter().map(String::from))
    .collect();

    if conditions.is_empty() {
        String::new()
    } else {
        format!(", {}", conditions.join(", "))
    }
}

fn price_condition(range: PriceRange) -> Option<String> {
    match (range.min, range.max) {
        (Some(min), Some(max)) => Some(format!("цена от {} до {}", rub(min), rub(max))),
        (Some(min), None) => Some(format!("цена от {}", rub(min))),
        (None, Some(max)) => Some(format!("цена до {}", rub(max))),
        (None, None) => None,
    }
}

pub(super) fn alert(alert: &Alert<'_>) -> String {
    match alert {
        Alert::Test => "Проверка связи: сюда будут приходить выгодные находки ваших слежек.".to_string(),
        Alert::Started { watch, deals: [] } => format!(
            "🔎 Слежка «{}» запущена ({}). Сейчас подходящих нет — напишу, как только появятся.",
            escape(&watch.name),
            goal_summary(watch)
        ),
        Alert::Started { watch, deals } => deals_message(
            &format!(
                "🔎 Слежка «{}» запущена ({}). Подходит прямо сейчас: {}. Дальше буду писать только о новых.",
                escape(&watch.name),
                goal_summary(watch),
                deals.len()
            ),
            deals,
        ),
        Alert::NewDeals { watch, deals } => deals_message(
            &format!(
                "🔔 «{}»: новые выгодные предложения — {}",
                escape(&watch.name),
                deals.len()
            ),
            deals,
        ),
        Alert::Failed { watch, reason } => format!(
            "⚠️ Слежка «{}» не смогла проверить {}: {}.\nПока это не исправлено, новых находок не будет. \
             Повторять это сообщение при каждой проверке не буду.",
            escape(&watch.name),
            watch.source.title(),
            escape(reason)
        ),
    }
}

fn goal_summary(watch: &Watch) -> String {
    let goal = match watch.goal {
        Goal::BelowMarket { min_discount_percent } => format!("ниже рынка от {min_discount_percent}%"),
        Goal::Resale { min_profit, .. } => format!("перепродажа: выгода от {}", rub(min_profit)),
    };
    let radius = watch
        .radius_km
        .map(|radius| format!(", до {radius} км от дома"))
        .unwrap_or_default();
    format!("{}, {goal}{radius}", watch.source.title())
}

/// Сколько находок влезает в одно сообщение — столько и показываем, остальные ждут в программе.
fn deals_message(header: &str, deals: &[Deal]) -> String {
    let mut length = header.chars().count();
    let entries: Vec<String> = deals
        .iter()
        .map(deal_entry)
        .take_while(|entry| {
            length += entry.chars().count();
            length <= MESSAGE_LIMIT - 100
        })
        .collect();
    let rest = deals.len() - entries.len();
    let tail = if rest > 0 {
        format!("\n\n…и ещё {rest} — во вкладке «Находки» в программе.")
    } else {
        String::new()
    };
    format!("{header}{}{tail}", entries.concat())
}

fn deal_entry(deal: &Deal) -> String {
    let resale = deal
        .resale_profit
        .map(|profit| format!("\nНа перепродаже ≈ {} выгоды", signed_rub(profit)))
        .unwrap_or_default();
    let whereabouts = whereabouts(&deal.offer);
    let whereabouts = if whereabouts.is_empty() {
        String::new()
    } else {
        format!("\n📍 {}", escape(&whereabouts))
    };
    let marks = marks_line(&deal.offer);
    let warning = if deal.suspicious {
        "\n⚠️ Дешевле половины рынка — проверьте продавца и состояние"
    } else {
        ""
    };
    // У расходников рядом с ценой пачки — цена за килограмм, и рынок назван тоже за килограмм.
    let (unit_price, market) = match deal.per_unit {
        Some(per_unit) => (format!(" · {}", per_unit.price), per_unit.market.to_string()),
        None => (String::new(), rub(deal.market_price)),
    };
    format!(
        "\n\n<b>{price}</b>{unit_price} — на {discount}% ниже рынка ({market}, по {sample} похожим){resale}{whereabouts}{marks}{warning}\n<a href=\"{url}\">{name}</a>",
        price = rub(deal.offer.price),
        discount = deal.discount_percent,
        sample = deal.market_sample,
        url = escape(&deal.offer.url),
        name = escape(&shorten(&deal.offer.name, NAME_LIMIT)),
    )
}

/// Отдельной строкой под предложением, если с плитки что-то известно.
fn marks_line(offer: &Offer) -> String {
    let marks = tile_marks(offer);
    if marks.is_empty() {
        String::new()
    } else {
        format!("\n{}", escape(&marks))
    }
}

/// «2.4 км от дома · Валовая ул. Павелецкая · 1 час назад» — что известно о месте и времени.
fn whereabouts(offer: &Offer) -> String {
    let distance = offer.distance_km.map(|km| format!("{km:.1} км от дома"));
    [distance, offer.place.clone(), offer.posted.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" · ")
}

fn shorten(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    text.chars().take(limit).collect::<String>() + "…"
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;
    use crate::hunt::PerUnit;
    use crate::quantity::{Unit, UnitPrice};

    fn findings(offers: Vec<Offer>) -> Findings {
        Findings {
            scanned: 80,
            suitable: offers
                .into_iter()
                .map(|offer| Finding {
                    offer,
                    unit_price: None,
                })
                .collect(),
            packed_in: None,
        }
    }

    fn powder(name: &str, price: u64, per_kilogram: Option<u64>) -> Finding {
        Finding {
            offer: Offer::sample(name, price),
            unit_price: per_kilogram.map(|rubles| UnitPrice {
                kopecks: rubles * 100,
                unit: Unit::Kilogram,
            }),
        }
    }

    #[test]
    fn per_kilogram_answer_shows_price_per_kilogram() {
        let found = Findings {
            scanned: 80,
            suitable: vec![
                powder("Порошок Persil 6 кг", 1400, Some(233)),
                powder("Порошок Persil", 100, None),
            ],
            packed_in: Some(Unit::Kilogram),
        };
        let text = super::findings(&SearchRequest::parse("стиральный порошок за кг"), &found, 10);

        assert!(text.starts_with("<b>стиральный порошок</b>\nПодходят 2 из 80, самые дешёвые за кг:"));
        assert!(text.contains("1. <b>1\u{a0}400\u{a0}₽</b> · 233\u{a0}₽/кг\n"));
        assert!(text.contains("2. <b>100\u{a0}₽</b>\n"));
        assert!(!text.contains("Сравнить по цене"));
    }

    #[test]
    fn pack_sizes_in_titles_suggest_comparing_per_unit() {
        let found = Findings {
            packed_in: Some(Unit::Kilogram),
            ..findings(vec![Offer::sample("Порошок Persil 3 кг", 750)])
        };
        let text = super::findings(&SearchRequest::parse("стиральный порошок"), &found, 10);
        assert!(text.ends_with("\nСравнить по цене за кг: <code>стиральный порошок за кг</code>"));

        let on_sale = super::findings(&SearchRequest::parse("стиральный порошок по скидке"), &found, 10);
        assert!(on_sale.ends_with("<code>стиральный порошок за кг по скидке</code>"));
    }

    #[test]
    fn asked_per_unit_without_pack_sizes_says_so() {
        let text = super::findings(
            &SearchRequest::parse("чайник за кг"),
            &findings(vec![Offer::sample("Чайник", 1990)]),
            10,
        );
        assert!(text.contains("Подходят 1 из 80, самые дешёвые (фасовки в кг в названиях нет):"));
    }

    #[test]
    fn header_names_the_order() {
        let header = |text: &str, offer: Offer| {
            let answer = super::findings(&SearchRequest::parse(text), &findings(vec![offer]), 10);
            answer.lines().nth(1).unwrap().to_string()
        };
        let on_sale = Offer {
            old_price: Some(2_000),
            ..Offer::sample("Чайник", 1_000)
        };

        assert_eq!(
            header("чайник по скидке", on_sale.clone()),
            "Подходят 1 из 80, с самой большой скидкой:"
        );
        assert_eq!(
            header("чайник по скидке", Offer::sample("Чайник", 1_000)),
            "Подходят 1 из 80, самые дешёвые (скидок нет ни у одного):"
        );
        assert_eq!(
            header("чайник цена-качество", on_sale),
            "Подходят 1 из 80, лучшие по цене и рейтингу:"
        );
    }

    fn deal(price: u64) -> Deal {
        Deal {
            watch_id: 1,
            watch_name: "Шуруповёрт".into(),
            found_at: chrono::Local::now(),
            offer: Offer {
                distance_km: Some(2.34),
                place: Some("Валовая ул. Павелецкая".into()),
                posted: Some("1 час назад".into()),
                ..Offer::sample("Шуруповерт Makita <DF333>", price)
            },
            market_price: 12_000,
            market_sample: 9,
            discount_percent: 45,
            resale_profit: Some(3_500),
            suspicious: false,
            per_unit: None,
        }
    }

    #[test]
    fn consumable_deal_names_price_and_market_per_kilogram() {
        let per_kilogram = |rubles: u64| UnitPrice {
            kopecks: rubles * 100,
            unit: Unit::Kilogram,
        };
        let powder = Deal {
            offer: Offer::sample("Стиральный порошок Persil 9 кг", 2_070),
            market_price: 2_880,
            market_sample: 6,
            discount_percent: 28,
            resale_profit: None,
            per_unit: Some(PerUnit {
                price: per_kilogram(230),
                market: per_kilogram(320),
            }),
            ..deal(0)
        };
        assert!(
            deal_entry(&powder)
                .contains("<b>2\u{a0}070\u{a0}₽</b> · 230\u{a0}₽/кг — на 28% ниже рынка (320\u{a0}₽/кг, по 6 похожим)")
        );
    }

    fn drill_watch() -> Watch {
        Watch {
            name: "Шуруповёрт".into(),
            query: "шуруповерт makita".into(),
            source: crate::marketplace::SourceKind::Avito,
            radius_km: Some(5),
            goal: Goal::Resale {
                min_profit: 2_000,
                costs: 500,
            },
            ..Watch::new_draft()
        }
    }

    #[test]
    fn started_watch_without_deals_promises_to_write_later() {
        let watch = drill_watch();
        assert_eq!(
            alert(&Alert::Started {
                watch: &watch,
                deals: &[]
            }),
            "🔎 Слежка «Шуруповёрт» запущена (Авито, перепродажа: выгода от 2\u{a0}000\u{a0}₽, до 5 км от дома). \
             Сейчас подходящих нет — напишу, как только появятся."
        );
    }

    #[test]
    fn new_deal_shows_market_profit_distance_and_warning() {
        let watch = drill_watch();
        let deals = [Deal {
            suspicious: true,
            ..deal(6_600)
        }];
        let text = alert(&Alert::NewDeals {
            watch: &watch,
            deals: &deals,
        });

        assert!(text.starts_with("🔔 «Шуруповёрт»: новые выгодные предложения — 1"));
        assert!(text.contains("<b>6\u{a0}600\u{a0}₽</b> — на 45% ниже рынка (12\u{a0}000\u{a0}₽, по 9 похожим)"));
        assert!(text.contains("На перепродаже ≈ 3\u{a0}500\u{a0}₽ выгоды"));
        assert!(text.contains("📍 2.3 км от дома · Валовая ул. Павелецкая · 1 час назад"));
        assert!(text.contains("⚠️ Дешевле половины рынка"));
        assert!(text.contains(">Шуруповерт Makita &lt;DF333&gt;</a>"));
    }

    #[test]
    fn broken_watch_says_why_and_promises_not_to_repeat() {
        let watch = drill_watch();
        let text = alert(&Alert::Failed {
            watch: &watch,
            reason: "Авито просит подтвердить, что вы не робот — пройдите проверку в окне браузера программы",
        });
        assert!(text.starts_with("⚠️ Слежка «Шуруповёрт» не смогла проверить Авито: Авито просит подтвердить"));
        assert!(text.contains("Повторять это сообщение при каждой проверке не буду."));
    }

    #[test]
    fn long_list_of_deals_fits_one_message() {
        let watch = drill_watch();
        let deals: Vec<Deal> = (0..200)
            .map(|i| Deal {
                offer: Offer::sample(&"Очень длинное название ".repeat(5), 5_000 + i),
                ..deal(0)
            })
            .collect();
        let text = alert(&Alert::NewDeals {
            watch: &watch,
            deals: &deals,
        });
        assert!(text.chars().count() <= MESSAGE_LIMIT);
        assert!(text.contains("во вкладке «Находки» в программе"));
    }

    #[test]
    fn shows_price_discount_rating_marks_and_escaped_name() {
        let offer = Offer {
            old_price: Some(7990),
            rating: Some(4.9),
            reviews: Some(8834),
            delivery: Some("Завтра".into()),
            sold_by_ozon: true,
            ..Offer::sample("Sony <WH-CH520>", 4364)
        };
        let text = super::findings(
            &SearchRequest::parse("наушники sony до 5000"),
            &findings(vec![offer]),
            10,
        );

        assert!(text.starts_with("<b>наушники sony</b>, цена до 5\u{a0}000\u{a0}₽\nПодходят 1 из 80"));
        assert!(text.contains("<b>4\u{a0}364\u{a0}₽</b> <s>7\u{a0}990\u{a0}₽</s> −45%"));
        assert!(text.contains(">Sony &lt;WH-CH520&gt;</a>"));
        assert!(text.contains("★ 4.9 · отзывов: 8834\nдоставка завтра · продаёт Ozon\n"));
    }

    #[test]
    fn header_lists_every_condition() {
        let offer = Offer {
            rating: Some(4.9),
            reviews: Some(8834),
            ..Offer::sample("Sony WH-CH520", 4364)
        };
        let request =
            SearchRequest::parse("наушники sony до 5000 рейтинг от 4.5 отзывов от 100 доставка завтра только ozon");
        let text = super::findings(&request, &findings(vec![offer]), 10);

        assert!(text.starts_with(
            "<b>наушники sony</b>, цена до 5\u{a0}000\u{a0}₽, рейтинг от 4.5, отзывов от 100, доставка до завтра, \
             только Ozon\nПодходят 1 из 80"
        ));
    }

    #[test]
    fn empty_answer_hint_depends_on_conditions() {
        let plain = super::findings(&SearchRequest::parse("наушники"), &findings(Vec::new()), 10);
        let strict = super::findings(
            &SearchRequest::parse("наушники рейтинг от 5"),
            &findings(Vec::new()),
            10,
        );

        assert!(plain.ends_with("в названии товара должны быть все."));
        assert!(strict.ends_with("или ослабить условия."));
    }

    #[test]
    fn shows_at_most_top_offers() {
        let offers = (1..=5).map(|price| Offer::sample("Наушники", price)).collect();
        let text = super::findings(&SearchRequest::parse("наушники"), &findings(offers), 3);
        assert!(text.contains("\n3. ") && !text.contains("\n4. "));
    }

    #[test]
    fn long_answer_fits_telegram_limit() {
        let offers = (1..=100)
            .map(|price| Offer::sample(&"Очень длинное название ".repeat(10), price))
            .collect();
        let text = super::findings(&SearchRequest::parse("название"), &findings(offers), 100);
        assert!(text.chars().count() <= MESSAGE_LIMIT);
        assert!(text.contains("\n1. "));
    }

    #[test]
    fn broken_parser_message_lists_problems_and_sample() {
        let broken = Health::Broken {
            problems: vec![Problem::TooFewOffers { found: 1, expected: 10 }, Problem::NoRatings],
            sample: vec![Offer::sample("", 4364)],
        };
        let text = health(&broken, "Ozon", "наушники");

        assert!(text.starts_with("Самопроверка Ozon: похоже, сломался разбор выдачи"));
        assert!(text.contains("Контрольный запрос «наушники»:\n— найдено товаров: 1, а должно быть не меньше 10\n— ни у одного товара нет рейтинга"));
        assert!(text.contains("Что пришло:\n1. 4\u{a0}364\u{a0}₽ — (без названия) — https://example.com/1"));
    }

    #[test]
    fn empty_sample_is_said_in_words() {
        let broken = Health::Broken {
            problems: vec![Problem::TooFewOffers { found: 0, expected: 10 }],
            sample: Vec::new(),
        };
        assert!(health(&broken, "Ozon", "наушники").contains("Что пришло:\nни одного товара"));
    }

    #[test]
    fn recovery_and_unavailability_messages() {
        assert_eq!(
            health(&Health::Working { offers: 80 }, "Ozon", "наушники"),
            "Самопроверка Ozon: снова всё в порядке — по контрольному запросу «наушники» найдено товаров: 80."
        );
        assert!(
            health(&Health::Unavailable(SourceError::Captcha), "Ozon", "наушники")
                .starts_with("Самопроверка Ozon не прошла. Ozon просит подтвердить, что вы не робот.")
        );
    }

    #[test]
    fn failures_name_the_source() {
        assert!(failure(&SourceError::Captcha, "Ozon").starts_with("Ozon просит подтвердить"));
        assert!(failure(&SourceError::NotReady, "Ozon").starts_with("Ozon не открылся"));
        assert_eq!(
            failure(&SourceError::Failed(anyhow!("нет сети")), "Ozon"),
            "Не получилось: нет сети"
        );
    }
}
