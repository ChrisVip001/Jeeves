//! Turns a successful, scoped FlightStats detail read into a first-answer receipt.

use super::{Lookup, minutes};
use chrono::NaiveDate;
use jeeves_loop::ToolOutcome;
use regex::Regex;
use reqwest::Url;
use std::sync::LazyLock;

const SEAT_GUIDANCE: &str = "要避开机翼，请打开航司值机页的座位图，选机翼前缘之前或后缘之后、且明确有舷窗的座位。未核实匹配的座位图，不能推荐具体排号、座位号或左右侧。";

#[derive(PartialEq, Eq)]
struct Record {
    flight: String,
    day: NaiveDate,
    id: String,
}

fn record(url: &Url, prefix: &str) -> Option<Record> {
    if url.scheme() != "https"
        || !matches!(
            url.host_str(),
            Some("www.flightstats.com" | "flightstats.com")
        )
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some()
        || url.fragment().is_some()
        || url.query_pairs().count() != 4
    {
        return None;
    }
    let (carrier, number) = url.path().strip_prefix(prefix)?.split_once('/')?;
    if carrier.len() != 2
        || !carrier
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        || !(1..=4).contains(&number.len())
        || !number.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    let param = |name| {
        let mut matching = url.query_pairs().filter(|(key, _)| key == name);
        let (_, value) = matching.next()?;
        (matching.next().is_none()
            && !value.is_empty()
            && value.bytes().all(|b| b.is_ascii_digit()))
        .then_some(value)
    };
    Some(Record {
        flight: format!("{carrier}{number}"),
        day: NaiveDate::from_ymd_opt(
            param("year")?.parse().ok()?,
            param("month")?.parse().ok()?,
            param("date")?.parse().ok()?,
        )?,
        id: param("flightId")?.into_owned(),
    })
}

#[derive(PartialEq, Eq)]
struct Verified {
    record: Record,
    departure: u16,
    source: String,
}

impl Lookup {
    pub(in crate::turn) fn evidence_reply(&self, calls: &[ToolOutcome]) -> String {
        let mut verified = calls.iter().filter_map(|call| self.verify(call));
        if let Some(first) = verified.next()
            && verified
                .all(|other| other.record == first.record && other.departure == first.departure)
        {
            return format!(
                "公开航班详情显示，{} {}到{}的 [{}]({}) 当地计划 {:02}:{:02} 起飞。这是符合你所述时段的候选航班，请和机票上的航班号核对。\n\n{SEAT_GUIDANCE}",
                self.day,
                self.origin,
                self.destination,
                first.record.flight,
                first.source,
                first.departure / 60,
                first.departure % 60,
            );
        }
        format!("未能核实你描述的航班。请提供机票上的航班号。\n\n{SEAT_GUIDANCE}")
    }

    fn verify(&self, call: &ToolOutcome) -> Option<Verified> {
        let (url, record, status) = self.scoped_read(call)?;
        if record.day != self.day || minutes(&self.time)?.abs_diff(status.departure) > 30 {
            return None;
        }
        Some(Verified {
            record,
            departure: status.departure,
            source: url.to_string(),
        })
    }

    pub(super) fn navigation(&self, call: &ToolOutcome) -> Option<String> {
        let (_, old, _) = self.scoped_read(call)?;
        if old.day == self.day {
            return None;
        }
        let (_, navigation) = call.output.split_once("Past and Upcoming Flights")?;
        let lines: Vec<_> = navigation
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .collect();
        let mut day = None;
        let mut links = std::collections::BTreeSet::new();
        let origin = city_key(&self.origin);
        let destination = city_key(&self.destination);
        let requested = minutes(&self.time)?;
        for (index, line) in lines.iter().enumerate() {
            if let Some(heading) = line.strip_prefix("Flights for ") {
                day = heading
                    .split_once(", ")
                    .and_then(|(_, d)| NaiveDate::parse_from_str(d, "%d-%b-%Y").ok());
                continue;
            }
            if day != Some(self.day) {
                continue;
            }
            let Some((clock, origin_code, origin_city, destination_code, destination_city)) =
                navigation_row(&lines, index)
            else {
                continue;
            };
            if airport(origin_code).is_none()
                || airport(destination_code).is_none()
                || origin_code == destination_code
                || city_key(origin_city) != origin
                || city_key(destination_city) != destination
                || requested.abs_diff(clock) > 30
            {
                continue;
            }
            let Some((url, _)) = line
                .split_once('<')
                .and_then(|(_, tail)| tail.split_once('>'))
            else {
                continue;
            };
            if let Ok(parsed) = Url::parse(url)
                && let Some(link) = record(&parsed, "/v2/flight-tracker/")
                && link.flight == old.flight
                && link.day == self.day
            {
                links.insert(url);
            }
        }
        let mut links = links.into_iter();
        let only = links.next()?;
        links.next().is_none().then(|| only.to_string())
    }

    fn scoped_read<'a>(&self, call: &'a ToolOutcome) -> Option<(Url, Record, Status<'a>)> {
        if call.name != "fetch_url" || !call.ok() {
            return None;
        }
        let url = Url::parse(call.input["url"].as_str()?).ok()?;
        let record = record(&url, "/v2/flight-tracker/")?;
        let status = status(&call.output)?;
        let origin = city_key(&self.origin);
        let destination = city_key(&self.destination);
        if status.flight.replace(' ', "") != record.flight
            || status.day != record.day
            || origin.is_empty()
            || destination.is_empty()
            || origin == destination
            || city_key(status.origin) != origin
            || city_key(status.destination) != destination
            || status.record != record
        {
            return None;
        }
        Some((url, record, status))
    }
}

fn navigation_row<'a>(
    lines: &[&'a str],
    link_index: usize,
) -> Option<(u16, &'a str, &'a str, &'a str, &'a str)> {
    let [
        departure,
        origin_code,
        origin,
        destination_code,
        destination,
        arrival,
    ] = lines.get(link_index.checked_sub(6)?..link_index)?
    else {
        return None;
    };
    local_clock(arrival)?;
    Some((
        local_clock(departure)?,
        origin_code,
        origin,
        destination_code,
        destination,
    ))
}

fn local_clock(line: &str) -> Option<u16> {
    let (clock, _) = line.split_once(' ')?;
    minutes(clock)
}

pub(super) fn explicit_tracker(message: &str) -> Option<&str> {
    static URL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"https://[^\s<>\[\]()、。，；！？（）]+").expect("static tracker URL pattern")
    });
    let mut urls = URL
        .find_iter(message)
        .map(|hit| hit.as_str().trim_end_matches(['.', ',', '!', ';']))
        .filter(|raw| {
            Url::parse(raw)
                .ok()
                .and_then(|url| record(&url, "/v2/flight-tracker/"))
                .is_some()
        });
    let only = urls.next()?;
    urls.next().is_none().then_some(only)
}

pub(super) fn city_key(city: &str) -> String {
    deunicode::deunicode(city)
        .bytes()
        .filter(u8::is_ascii_alphanumeric)
        .map(|b| char::from(b.to_ascii_lowercase()))
        .collect()
}

struct Status<'a> {
    flight: &'a str,
    origin: &'a str,
    destination: &'a str,
    day: NaiveDate,
    departure: u16,
    record: Record,
}

fn status(body: &str) -> Option<Status<'_>> {
    let lines: Vec<_> = body
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect();
    let scope = lines
        .iter()
        .position(|line| *line == "Past and Upcoming Flights")
        .unwrap_or(lines.len());
    let start = unique_line(&lines[..scope], "Flight Status")? + 1;
    let main = &lines[start..scope];
    let end = unique_line(main, "Flight Tracker")?;
    let main = &main[..end];
    let [
        flight,
        _airline,
        origin_code,
        origin,
        destination_code,
        destination,
        state,
        ..,
    ] = main
    else {
        return None;
    };
    let (origin_airport, destination_airport) = (airport(origin_code)?, airport(destination_code)?);
    if origin_airport == destination_airport
        || state.eq_ignore_ascii_case("Canceled")
        || state.eq_ignore_ascii_case("Cancelled")
    {
        return None;
    }
    let departure = unique_line(main, "Flight Departure Times")?;
    let arrival = unique_line(main, "Flight Arrival Times")?;
    let [_, day, "Scheduled", clock, ..] = main.get(departure..arrival)? else {
        return None;
    };
    let at_airport = |section: &[&str], code, city: &str| {
        section.windows(2).any(|rows| {
            airport(rows[0]) == Some(code)
                && rows[1]
                    .split_once(',')
                    .is_some_and(|(name, _)| name == city)
        })
    };
    if !at_airport(main.get(7..departure)?, origin_airport, origin)
        || !at_airport(
            main.get(departure..arrival)?,
            destination_airport,
            destination,
        )
    {
        return None;
    }
    let mut links = main[arrival..].iter().filter_map(|line| {
        let rest = line.strip_prefix("VIEW FLIGHT DETAILS <")?;
        let (source, _) = rest.split_once('>')?;
        Url::parse(source).ok()
    });
    let detail = links.next()?;
    if links.next().is_some() {
        return None;
    }
    Some(Status {
        flight,
        origin,
        destination,
        day: NaiveDate::parse_from_str(day, "%d-%b-%Y").ok()?,
        departure: local_clock(clock)?,
        record: record(&detail, "/v2/flight-details/")?,
    })
}

fn unique_line(lines: &[&str], label: &str) -> Option<usize> {
    let mut matches = lines.iter().enumerate().filter(|(_, line)| **line == label);
    let (index, _) = matches.next()?;
    matches.next().is_none().then_some(index)
}

fn airport(line: &str) -> Option<&str> {
    let code = line.split_ascii_whitespace().next()?;
    (code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())).then_some(code)
}
