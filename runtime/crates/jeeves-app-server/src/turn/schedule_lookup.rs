//! Narrow, public-only lookup for a remembered domestic flight departure.

use chrono::{Days, NaiveDate};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

static ROUTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"从\s*([\p{Han}]{2,6}?)\s*(?:飞往|飞到|到)\s*([\p{Han}]{2,6}?)\s*(?:的\s*)?(?:航班|飞机)",
    )
    .expect("static flight route pattern")
});
static DATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[0-9]{4}-[0-9]{2}-[0-9]{2})|今天|明天|后天").expect("static date pattern")
});
static CLOCK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[01]?[0-9]|2[0-3])[:：][0-5][0-9]|(?:[01]?[0-9]|2[0-3]|[一二两三四五六七八九十]{1,3})[点點](?:[半整]|[0-5]?[0-9]分)?")
        .expect("static clock pattern")
});

pub(super) const EVIDENCE_NOTE: &str = concat!(
    "\n\nThe runtime scheduled one public timetable lookup for exact and nearby departure leads. ",
    "Read both host results if present; the second is a single flight-number plus exact-date check, not confirmation. ",
    "Do not repeat the route or flight-number search. If only the first result is present and it yields a credible flight number, ",
    "use at most one follow-up search targeted to that flight number and exact local date, not a broader route query. ",
    "Check the most relevant accessible source rather than combining fragments. Cite a candidate with a clickable source link and the ",
    "target local date, direction, departure time and flight number from the same row, or a dated ",
    "timetable covering that weekday. If evidence lacks the target day, label it an undated lead, ",
    "not an itinerary or a confirmed candidate. Do not assume two different numbers are a codeshare. ",
    "A source that omits the requested departure does not prove no such flight exists. If no lead is ",
    "supported, say that plainly without listing unrelated flights. Without a matching seat map, give ",
    "only conditional guidance for avoiding the wing and do not guess a row or which side faces the sun.",
);

pub(super) struct Lookup {
    origin: String,
    destination: String,
    day: NaiveDate,
    time: String,
}

impl Lookup {
    fn initial(&self) -> Value {
        json!({
            "query": format!("{} 到 {} 航班 {} {} 起飞 航班号", self.origin, self.destination, self.day, self.time),
            "approximate_time": self.time,
            "max_results": 4
        })
    }

    pub(super) fn plan(self) -> jeeves_loop::ScheduleSearch {
        jeeves_loop::ScheduleSearch {
            initial: self.initial(),
            refine: Box::new(move |output| self.refinement(output)),
        }
    }

    // A lead must bind one direction, one flight number and one nearby clock on the same
    // line. This is a query selector, not proof: excerpts can be stale or mistaken.
    // Only the validated flight number goes back to search; never send excerpt text or URLs.
    fn refinement(&self, output: &str) -> Option<Value> {
        static FLIGHT: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"(?-u:\b)([A-Z][A-Z0-9]|[0-9][A-Z])[ \t]?[0-9]{3,4}(?-u:\b)").unwrap()
        });
        static OTHER_DATE: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(
            r"(?i)[0-9]{1,2}月[0-9]{1,2}|[0-9]{1,4}/[0-9]{1,2}|(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)[a-z]*[ .]+[0-9]{1,2}|[0-9]{1,2}[ .]+(?:Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec)\b"
        ).unwrap()
        });
        let direction = Regex::new(&format!(
            r"{}\s*(?:飞往|飞到|到|至|→|[-–—>]+)\s*{}",
            self.origin, self.destination
        ))
        .ok()?;
        let reverse = Regex::new(&format!(
            r"{}\s*(?:飞往|飞到|到|至|→|[-–—>]+)\s*{}",
            self.destination, self.origin
        ))
        .ok()?;
        let requested = minutes(&self.time)?;
        let date = self.day.to_string();
        let mut candidates = std::collections::BTreeMap::new();
        for line in output.lines().take(400) {
            // Source metadata, search query echoes and embedded links are not timetable rows.
            let line = line.trim();
            if line.len() > 640
                || [
                    "URL:",
                    "Source ",
                    "Published:",
                    "Retrieved ",
                    "Web results ",
                ]
                .iter()
                .any(|prefix| line.starts_with(prefix))
                || line.contains("://")
                || direction.find_iter(line).count() != 1
                || reverse.is_match(line)
                || DATE.find_iter(line).any(|d| d.as_str() != date)
                || OTHER_DATE.is_match(line)
                || ["取消", "经停", "到达", "actual arrival"]
                    .iter()
                    .any(|w| line.contains(w))
            {
                continue;
            }
            let mut times = CLOCK.find_iter(line);
            let Some(time) = times.next().and_then(|t| minutes(t.as_str())) else {
                continue;
            };
            if times.next().is_some() || requested.abs_diff(time) > 30 {
                continue;
            }
            let mut numbers = FLIGHT.find_iter(line);
            let Some(number) = numbers.next() else {
                continue;
            };
            if numbers.next().is_some() {
                continue;
            }
            let id = number.as_str().replace([' ', '\t'], "");
            let distance = requested.abs_diff(time);
            candidates
                .entry(id)
                .and_modify(|min: &mut u16| *min = (*min).min(distance))
                .or_insert(distance);
        }
        let nearest = candidates.values().min()?;
        let mut best = candidates
            .iter()
            .filter(|(_, distance)| *distance == nearest);
        let (flight, _) = best.next()?;
        if best.next().is_some() {
            return None;
        }
        Some(json!({
            "query": format!("{flight} {} 到 {} {} 航班动态 当日实际计划起飞 航司 机场", self.origin, self.destination, self.day),
            "max_results": 3
        }))
    }
}

fn minutes(value: &str) -> Option<u16> {
    let (h, m) = value.split_once([':', '：'])?;
    let h: u16 = h.parse().ok()?;
    let m: u16 = m.parse().ok()?;
    (h < 24 && m < 60).then(|| h * 60 + m)
}

pub(super) fn request(message: &str, today: NaiveDate) -> Option<Lookup> {
    if !["订了", "买了", "已订"].iter().any(|s| message.contains(s))
        || !["座位", "靠窗", "选座", "机翼"]
            .iter()
            .any(|s| message.contains(s))
        || [
            "最晚",
            "最早",
            "必须",
            "不能",
            "不得",
            "不早于",
            "不晚于",
            "只要",
            "以前",
            "以后",
            "之前",
            "之后",
            "准时",
            "正好",
            "恰好",
            "严格",
            "不要查",
            "不用查",
            "不需要查",
            "别查",
            "别搜",
            "不联网",
            "别联网",
            "不要联网",
            "不用联网",
            "不要搜索",
            "不搜索",
            "只用",
        ]
        .iter()
        .any(|s| message.contains(s))
    {
        return None;
    }
    let route = ROUTE.captures(message)?;
    if ROUTE.find_iter(message).count() != 1 || route[1] == route[2] {
        return None;
    }
    let mut dates = DATE.find_iter(message);
    let day = match dates.next()?.as_str() {
        "今天" => today,
        "明天" => today.checked_add_days(Days::new(1))?,
        "后天" => today.checked_add_days(Days::new(2))?,
        date => NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?,
    };
    if dates.next().is_some() {
        return None;
    }
    let mut clocks = CLOCK.find_iter(message);
    let clock = clocks.next()?;
    if clocks.next().is_some() || clock.end() > route.get(0)?.start() {
        return None;
    }
    let time = clock_value(clock.as_str(), &message[..clock.start()])?;
    Some(Lookup {
        origin: route[1].to_string(),
        destination: route[2].to_string(),
        day,
        time,
    })
}

fn clock_value(value: &str, prefix: &str) -> Option<String> {
    let (mut hour, minute) = if let Some((h, m)) = value.split_once([':', '：']) {
        (h.parse::<u32>().ok()?, m.parse::<u32>().ok()?)
    } else {
        let (h, m) = value.split_once(['点', '點'])?;
        let minute = match m {
            "半" => 30,
            "整" | "" => 0,
            other => other.strip_suffix('分')?.parse::<u32>().ok()?,
        };
        (h.parse::<u32>().ok().or_else(|| han_hour(h))?, minute)
    };
    let prefix = prefix.trim_end();
    if ["下午", "傍晚", "晚上"].iter().any(|p| prefix.ends_with(p)) && hour < 12 {
        hour += 12;
    }
    if ["凌晨", "早晨", "早上", "上午"]
        .iter()
        .any(|p| prefix.ends_with(p))
        && hour == 12
    {
        return None;
    }
    (hour < 24 && minute < 60).then(|| format!("{hour:02}:{minute:02}"))
}

fn han_hour(text: &str) -> Option<u32> {
    fn digit(c: char) -> Option<u32> {
        "一二三四五六七八九"
            .find(c)
            .map(|i| (i / 3 + 1) as u32)
            .or_else(|| (c == '两').then_some(2))
    }
    if let Some((left, right)) = text.split_once('十') {
        let tens = if left.is_empty() {
            1
        } else {
            digit(single(left)?)?
        };
        let ones = if right.is_empty() {
            0
        } else {
            digit(single(right)?)?
        };
        Some(tens * 10 + ones)
    } else {
        digit(single(text)?)
    }
}

fn single(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn makes_one_dated_route_query_without_sending_booking_details() {
        let day = NaiveDate::from_ymd_opt(2030, 11, 7).unwrap();
        let ask = "我订了明天早晨六点半从甲城到乙城的飞机，座位怎么选？订单 A1245678";
        assert_eq!(
            request(ask, day).map(|lookup| lookup.initial()),
            Some(json!({
                "query": "甲城 到 乙城 航班 2030-11-08 06:30 起飞 航班号",
                "approximate_time":"06:30", "max_results":4
            }))
        );
        let ask = "我买了2030-11-10晚上八点整从甲城飞往乙城的航班，怎么选靠窗？";
        assert_eq!(
            request(ask, day).unwrap().initial()["approximate_time"],
            "20:00"
        );
        assert_eq!(
            request("我已订后天09:05从甲城到乙城的航班，想选座", day)
                .unwrap()
                .initial()["query"],
            "甲城 到 乙城 航班 2030-11-09 09:05 起飞 航班号"
        );
        for ask in [
            "查明天早晨六点半从甲城到乙城的飞机",
            "我订了明天最晚六点半从甲城到乙城的飞机，怎么选座？",
            "我订了明天六点半从甲城到乙城的飞机，怎么选座？别联网查",
            "我订了今天和明天六点半从甲城到乙城的飞机，选座",
            "我订了明天六点半从甲城到乙城的飞机，选座。改到八点",
            "我订了明天六点半从甲城到乙城的飞机，怎么付费？",
            "我订了明天06:30从甲城到甲城的飞机，怎么选座？",
        ] {
            assert!(request(ask, day).is_none(), "{ask}");
        }
    }

    #[test]
    fn refines_only_a_unique_nearby_flight_from_one_row() {
        let today = NaiveDate::from_ymd_opt(2030, 11, 7).unwrap();
        let lookup = request(
            "我订了明天六点半从甲城到乙城的飞机，怎么选座？订单 A1245678",
            today,
        )
        .unwrap();
        let decoys = "乙城到甲城 QZ456 06:30\n2030-11-09 甲城到乙城 QZ789 06:30\nNov 9 甲城到乙城 QZ890 06:30\n甲城到乙城 QZ111 05:59\n甲城到乙城 QZ222 06:30 08:10\n甲城到乙城 QZ333 / QZ444 06:30";
        assert!(lookup.refinement(decoys).is_none());
        let result = lookup
            .refinement(&format!(
                "{decoys}\n甲城到乙城 QZ 123 06:25\n甲城→乙城 QZ123 06:25"
            ))
            .unwrap();
        assert_eq!(
            result["query"],
            "QZ123 甲城 到 乙城 2030-11-08 航班动态 当日实际计划起飞 航司 机场"
        );
        assert!(
            lookup
                .refinement("甲城→乙城 QZ123 06:25\n甲城→乙城 QZ456 06:35")
                .is_none()
        );
        assert!(lookup.refinement("甲城→乙城\nQZ123 06:25").is_none());
        assert!(
            lookup
                .refinement("Source 1: 甲城→乙城 QZ123 06:25")
                .is_none()
        );
    }
}
