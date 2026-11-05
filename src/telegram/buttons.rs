use std::collections::{HashSet, VecDeque};
use std::time::{SystemTime, UNIX_EPOCH};

use teloxide::types::InlineKeyboardMarkup;

use crate::hunt::Miss;
use crate::search::{Findings, Order, SearchRequest};
use crate::telegram::render;

/// Порог кнопки «Рейтинг от 4.5».
const GOOD_RATING: f32 = 4.5;
/// Сколько последних ответов отзываются на кнопки: каждый держит всю выдачу в памяти.
const REMEMBERED_ANSWERS: usize = 20;

/// Нажатая кнопка. Telegram возвращает только строку до 64 байт, поэтому ответ на поиск кнопка называет ключом,
/// а находку — слежкой и отпечатком ссылки.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Press {
    More(u64),
    ByDiscount(u64),
    GoodRating(u64),
    Watch(u64),
    Miss { watch_id: u64, deal: u64 },
    Reason { watch_id: u64, deal: u64, miss: Miss },
}

impl Press {
    pub(super) fn data(self) -> String {
        match self {
            Press::More(key) => format!("more:{key}"),
            Press::ByDiscount(key) => format!("discount:{key}"),
            Press::GoodRating(key) => format!("rating:{key}"),
            Press::Watch(key) => format!("watch:{key}"),
            Press::Miss { watch_id, deal } => format!("miss:{watch_id}:{deal:x}"),
            Press::Reason { watch_id, deal, miss } => {
                let reason = Miss::ALL.iter().position(|&other| other == miss).unwrap_or_default();
                format!("why:{watch_id}:{deal:x}:{reason}")
            }
        }
    }

    fn parse(data: &str) -> Option<Self> {
        let mut parts = data.split(':');
        let action = parts.next()?;
        let first: u64 = parts.next()?.parse().ok()?;
        let press = match action {
            "more" => Press::More(first),
            "discount" => Press::ByDiscount(first),
            "rating" => Press::GoodRating(first),
            "watch" => Press::Watch(first),
            "miss" => Press::Miss {
                watch_id: first,
                deal: u64::from_str_radix(parts.next()?, 16).ok()?,
            },
            "why" => Press::Reason {
                watch_id: first,
                deal: u64::from_str_radix(parts.next()?, 16).ok()?,
                miss: *Miss::ALL.get(parts.next()?.parse::<usize>().ok()?)?,
            },
            _ => return None,
        };
        parts.next().is_none().then_some(press)
    }
}

/// Что делать с нажатием. Кнопки видны всем, кому переслали сообщение, поэтому доступ проверяется и здесь.
#[derive(Debug, PartialEq)]
pub(super) enum Pressed {
    Denied,
    Unknown,
    Button(Press),
}

pub(super) fn pressed(data: Option<&str>, user_id: u64, allowed_users: &HashSet<u64>) -> Pressed {
    if !allowed_users.contains(&user_id) {
        return Pressed::Denied;
    }
    data.and_then(Press::parse).map_or(Pressed::Unknown, Pressed::Button)
}

/// Отпечаток ссылки находки: сама ссылка в кнопку не влезает. FNV-1a, а не `DefaultHasher`: кнопки живут в чате
/// дольше программы, и отпечаток не должен меняться от сборки к сборке.
pub(super) fn deal_key(url: &str) -> u64 {
    url.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Ответ на поиск, который можно листать и пересобирать кнопками, не заходя на площадку снова.
pub(super) struct Answer {
    pub(super) request: SearchRequest,
    findings: Findings,
    from: usize,
    shown: usize,
}

impl Answer {
    pub(super) fn new(request: SearchRequest, findings: Findings) -> Self {
        Self {
            request,
            findings,
            from: 0,
            shown: 0,
        }
    }

    pub(super) fn apply(&mut self, press: Press) {
        match press {
            // Двойное нажатие: вторая кнопка ещё на экране, а листать уже некуда.
            Press::More(_) if self.from + self.shown < self.findings.suitable.len() => self.from += self.shown,
            Press::ByDiscount(_) => {
                self.request.order = Order::BiggestDiscount;
                self.rearrange();
            }
            Press::GoodRating(_) => {
                self.request.min_rating = Some(self.request.min_rating.map_or(GOOD_RATING, |own| own.max(GOOD_RATING)));
                self.rearrange();
            }
            Press::More(_) | Press::Watch(_) | Press::Miss { .. } | Press::Reason { .. } => {}
        }
    }

    fn rearrange(&mut self) {
        self.findings.refine(&self.request);
        self.from = 0;
    }

    /// Текст ответа и кнопки под ним для текущего места в выдаче.
    pub(super) fn show(&mut self, key: u64, top: usize) -> (String, InlineKeyboardMarkup) {
        let page = render::page(&self.request, &self.findings, self.from, top);
        self.shown = page.shown;
        let left = self.findings.suitable.len().saturating_sub(self.from + self.shown);
        let actions = render::SearchActions {
            more: (left > 0).then_some(left.min(top)),
            by_discount: self.request.order != Order::BiggestDiscount && !self.findings.suitable.is_empty(),
            good_rating: self.request.min_rating.is_none_or(|own| own < GOOD_RATING)
                && !self.findings.suitable.is_empty(),
        };
        (page.text, render::search_keyboard(key, &actions))
    }
}

/// Последние ответы по ключам. Ключи начинаются со времени запуска: кнопка из чата, оставшаяся от прошлого запуска
/// программы, не должна попасть в новый ответ с тем же номером.
pub(super) struct Answers {
    next_key: u64,
    answers: VecDeque<(u64, Answer)>,
}

impl Answers {
    pub(super) fn new() -> Self {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        Self::starting_at(started)
    }

    fn starting_at(next_key: u64) -> Self {
        Self {
            next_key,
            answers: VecDeque::new(),
        }
    }

    pub(super) fn add(&mut self, answer: Answer) -> u64 {
        let key = self.next_key;
        self.next_key += 1;
        if self.answers.len() == REMEMBERED_ANSWERS {
            self.answers.pop_front();
        }
        self.answers.push_back((key, answer));
        key
    }

    pub(super) fn get_mut(&mut self, key: u64) -> Option<&mut Answer> {
        self.answers
            .iter_mut()
            .find(|(own, _)| *own == key)
            .map(|(_, answer)| answer)
    }
}

#[cfg(test)]
mod tests {
    use teloxide::types::InlineKeyboardButtonKind;

    use super::*;
    use crate::marketplace::SourceKind;
    use crate::offer::Offer;
    use crate::search::Finding;

    const OWNER: u64 = 123_456_789;

    fn answer(prices: std::ops::Range<u64>) -> Answer {
        let suitable = prices
            .map(|price| Finding {
                source: SourceKind::Ozon,
                offer: Offer {
                    url: format!("https://www.ozon.ru/product/{price}/"),
                    rating: Some(if price % 2 == 0 { 4.8 } else { 4.0 }),
                    old_price: Some(price * 2 + 1000 - price % 3 * 500),
                    ..Offer::sample("Наушники Sony", price)
                },
                unit_price: None,
                suspicious: false,
            })
            .collect();
        Answer::new(
            SearchRequest::parse("наушники sony"),
            Findings {
                scanned: 80,
                suitable,
                packed_in: None,
                failed: Vec::new(),
            },
        )
    }

    #[test]
    fn second_press_of_more_at_the_end_keeps_the_last_page() {
        let mut answer = answer(1000..1012);
        answer.show(7, 10);
        answer.apply(Press::More(7));
        answer.show(7, 10);

        answer.apply(Press::More(7));

        let (text, _) = answer.show(7, 10);
        assert!(text.contains("\n11. "), "{text}");
    }

    fn labels(keyboard: &InlineKeyboardMarkup) -> Vec<String> {
        keyboard
            .inline_keyboard
            .iter()
            .flatten()
            .filter(|button| matches!(button.kind, InlineKeyboardButtonKind::CallbackData(_)))
            .map(|button| button.text.clone())
            .collect()
    }

    #[test]
    fn every_press_survives_the_trip_through_telegram_within_its_limit() {
        let far = u64::MAX;
        for press in [
            Press::More(far),
            Press::ByDiscount(far),
            Press::GoodRating(far),
            Press::Watch(far),
            Press::Miss {
                watch_id: far,
                deal: far,
            },
            Press::Reason {
                watch_id: far,
                deal: far,
                miss: Miss::Sold,
            },
        ] {
            assert!(press.data().len() <= 64, "{}", press.data());
            assert_eq!(Press::parse(&press.data()), Some(press));
        }
    }

    #[test]
    fn broken_data_is_not_a_press() {
        for data in ["", "more", "more:x", "more:1:2", "miss:1", "why:1:ff:9", "fly:1"] {
            assert_eq!(Press::parse(data), None, "{data}");
        }
    }

    #[test]
    fn strangers_cannot_press_a_forwarded_button() {
        let allowed = HashSet::from([OWNER]);
        assert_eq!(pressed(Some("more:1"), 42, &allowed), Pressed::Denied);
        assert_eq!(
            pressed(Some("more:1"), OWNER, &allowed),
            Pressed::Button(Press::More(1))
        );
        assert_eq!(pressed(Some("старая кнопка"), OWNER, &allowed), Pressed::Unknown);
    }

    #[test]
    fn deal_key_does_not_change_between_builds() {
        assert_eq!(
            deal_key("https://www.ozon.ru/product/1/"),
            deal_key("https://www.ozon.ru/product/1/")
        );
        assert_ne!(
            deal_key("https://www.ozon.ru/product/1/"),
            deal_key("https://www.ozon.ru/product/2/")
        );
        assert_eq!(deal_key("a"), 0xaf63_dc4c_8601_ec8c, "опубликованное значение FNV-1a");
    }

    #[test]
    fn more_shows_the_next_offers_and_disappears_at_the_end() {
        let mut answer = answer(1000..1025);

        let (first, keyboard) = answer.show(7, 10);
        assert!(first.contains("\n1. ") && !first.contains("\n11. "), "{first}");
        assert!(labels(&keyboard).contains(&"Ещё 10".to_string()));

        answer.apply(Press::More(7));
        let (second, _) = answer.show(7, 10);
        assert!(second.contains("\n11. ") && !second.contains("\n1. "), "{second}");

        answer.apply(Press::More(7));
        let (third, keyboard) = answer.show(7, 10);
        assert!(third.contains("\n25. "), "{third}");
        assert!(!labels(&keyboard).iter().any(|label| label.starts_with("Ещё")));
    }

    #[test]
    fn last_portion_names_how_many_are_left() {
        let mut answer = answer(1000..1013);
        let (_, keyboard) = answer.show(7, 10);
        assert!(labels(&keyboard).contains(&"Ещё 3".to_string()));
    }

    #[test]
    fn good_rating_narrows_the_answer_from_the_start_and_hides_its_button() {
        let mut answer = answer(1000..1025);
        answer.show(7, 10);
        answer.apply(Press::More(7));

        answer.apply(Press::GoodRating(7));

        assert_eq!(answer.request.min_rating, Some(4.5));
        assert_eq!(answer.findings.suitable.len(), 13);
        let (text, keyboard) = answer.show(7, 10);
        assert!(text.contains("\n1. "), "{text}");
        assert!(!labels(&keyboard).contains(&"Рейтинг от 4.5".to_string()));
    }

    #[test]
    fn own_higher_rating_is_not_lowered() {
        let mut answer = answer(1000..1005);
        answer.request.min_rating = Some(4.7);
        answer.apply(Press::GoodRating(7));
        assert_eq!(answer.request.min_rating, Some(4.7));
    }

    #[test]
    fn by_discount_rearranges_and_hides_its_button() {
        let mut answer = answer(1000..1006);
        let cheapest = answer.findings.suitable[0].offer.price;

        answer.apply(Press::ByDiscount(7));

        assert_eq!(answer.request.order, Order::BiggestDiscount);
        assert_ne!(answer.findings.suitable[0].offer.price, cheapest);
        let (_, keyboard) = answer.show(7, 10);
        assert!(!labels(&keyboard).contains(&"По скидке".to_string()));
    }

    #[test]
    fn empty_answer_offers_only_to_watch() {
        let mut answer = answer(0..0);
        let (_, keyboard) = answer.show(7, 10);
        assert_eq!(labels(&keyboard), ["Следить"]);
    }

    #[test]
    fn old_answers_are_forgotten_and_keys_are_not_reused() {
        let mut answers = Answers::starting_at(100);
        let first = answers.add(answer(1000..1001));
        for _ in 0..REMEMBERED_ANSWERS {
            answers.add(answer(1000..1001));
        }
        assert!(answers.get_mut(first).is_none());
        assert!(answers.get_mut(first + 1).is_some());
    }
}
