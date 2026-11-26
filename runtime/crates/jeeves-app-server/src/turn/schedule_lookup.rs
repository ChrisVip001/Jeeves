//! Narrow, public-only lookup for a remembered domestic flight departure.

use chrono::{Days, NaiveDate};
use jeeves_loop::{ScheduleSearch, ScheduleTool};
use regex::Regex;
use serde_json::{Value, json};
use std::sync::LazyLock;

mod flight_status;

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
    Regex::new(r"(?:[01]?[0-9]|2[0-3])[:：][0-5][0-9]|(?:[01]?[0-9]|2[0-3]|[一二两三四五六七八九十]{1,3})[ \t]*[点點](?:[ \t]*(?:[半整]|[0-5]?[0-9]分))?")
        .expect("static clock pattern")
});

pub(super) const EVIDENCE_NOTE: &str = concat!(
    "\n\nThe runtime may read the supplied FlightStats tracker or search for exact and nearby public departure leads. ",
    "Read the host results. Search and timetable results are only leads, not confirmation. ",
    "If a tracking page is for another day, look for a link under the requested date and matching route. ",
    "Open that exact link, preserving the provider's full path and query parameters; do not construct a URL or cite the old page as the requested flight. ",
    "Distinguish scheduled from actual times. Do not use an old day's actual departure, aircraft or layout for another date. ",
    "If fetch_url reports a DNS failure, that host was not read. Do not retry the same host during this turn or build a substitute link. ",
    "Do not confuse a search snippet with a successful fetch. If two host searches found no nearby lead, do not repeat their queries or list out-of-window flights. ",
    "State the missing evidence in the user's language, with one next step, without weather or travel speculation. ",
    "If you see a credible nearby flight number and no flight-number search has run, use one FlightStats search for that number and the target date. ",
    "The search may return an older FlightStats tracker. Open it to discover the site's original dated link. ",
    "If tool evidence already includes the verified dated detail, wrap up without more calls. ",
    "The first reply can confirm a candidate only from a successfully fetched dated flight tracker detail, not a timetable or navigation entry. ",
    "On FlightStats use the dated tracker URL with flightId and read its main Flight Status, not the Past and Upcoming Flights section. ",
    "Verify the target local date, direction, scheduled departure and flight number together. ",
    "A verified public flight is still not confirmation of the user's booking. ",
    "A source that omits the requested departure does not prove no such flight exists. If no lead is ",
    "supported, say that plainly without listing unrelated flights. Without a matching seat map, give ",
    "only conditional guidance for avoiding the wing. For this lookup leave specific row numbers, seat codes, and left/right advice to the airline seat map; do not name them.",
);

#[derive(Clone)]
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

    pub(super) fn plan(
        &self,
        message: &str,
        can_search: bool,
        can_fetch: bool,
    ) -> Option<ScheduleSearch> {
        let initial_url = can_fetch
            .then(|| flight_status::explicit_tracker(message))
            .flatten();
        let (initial, refine) = if let Some(url) = initial_url {
            ((ScheduleTool::Fetch, json!({"url": url})), None)
        } else if can_search {
            let lookup = self.clone();
            let refine: jeeves_loop::ScheduleRefinement = Box::new(move |output| {
                lookup
                    .refinement(output)
                    .or_else(|| lookup.departure_board())
            });
            ((ScheduleTool::Search, self.initial()), Some(refine))
        } else {
            return None;
        };
        let lookup = self.clone();
        Some(ScheduleSearch {
            initial,
            refine,
            follow: Box::new(move |read| lookup.navigation(read).map(|url| json!({"url": url}))),
        })
    }

    fn departure_board(&self) -> Option<Value> {
        let minute = minutes(&self.time)?;
        let lo = minute.saturating_sub(30);
        let hi = (minute + 30).min(1439);
        Some(json!({
            "query": format!("{} to {} direct flight schedule departures {} {:02}:{:02}-{:02}:{:02}", flight_status::city_key(&self.origin), flight_status::city_key(&self.destination), self.day, lo / 60, lo % 60, hi / 60, hi % 60),
            "max_results": 5
        }))
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
        let h = h.trim_end();
        let minute = match m.trim_start() {
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

    #[test]
    fn searches_departure_board_once_when_the_initial_rows_do_not_match() {
        let today = NaiveDate::from_ymd_opt(2030, 11, 7).unwrap();
        let plan = request(
            "我订了明天00:10从甲城到乙城的飞机，怎么选座？订单 A1245678",
            today,
        )
        .unwrap()
        .plan("", true, true)
        .unwrap();
        let result = (plan.refine.unwrap())("2030-11-07 甲城到乙城 QZ123 00:10").unwrap();
        assert_eq!(
            result,
            json!({
                "query": "jiacheng to yicheng direct flight schedule departures 2030-11-08 00:00-00:40",
                "max_results": 5
            })
        );
    }
}
