use super::*;

#[test]
fn a_weekly_event_keeps_its_wall_time_across_dst() {
    let ics = "BEGIN:VCALENDAR\r\nVERSION:2.0\r\nBEGIN:VEVENT\r\nUID:weekly\r\nSUMMARY:Standup\r\nDTSTART;TZID=America/New_York:20240304T090000\r\nDTEND;TZID=America/New_York:20240304T093000\r\nRRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT\r\nEND:VCALENDAR\r\n";
    let start = "2024-03-01T00:00:00Z".parse().unwrap();
    let end = "2024-04-01T00:00:00Z".parse().unwrap();
    let events = events_in_range(ics, start, end, "UTC").unwrap();
    assert_eq!(events.len(), 3);
    let starts: Vec<_> = events.iter().map(|e| e.start.clone()).collect();
    assert_eq!(
        starts,
        [
            "2024-03-04T14:00:00+00:00",
            "2024-03-11T13:00:00+00:00",
            "2024-03-18T13:00:00+00:00",
        ]
    );
    assert!(events.iter().all(|e| e.timezone == "UTC"));
}

#[test]
fn an_all_day_event_preserves_its_date_across_timezones() {
    let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:holiday\nSUMMARY:Founders Day\nDTSTART;VALUE=DATE:20241101\nDTEND;VALUE=DATE:20241102\nEND:VEVENT\nEND:VCALENDAR\n";
    let events = events_in_range(
        ics,
        "2024-10-31T00:00:00Z".parse().unwrap(),
        "2024-11-03T00:00:00Z".parse().unwrap(),
        "Asia/Shanghai",
    )
    .unwrap();
    assert_eq!(events.len(), 1);
    assert!(events[0].all_day);
    assert_eq!(events[0].start_local, "2024-11-01T00:00:00+08:00");
    assert_eq!(events[0].end_local, "2024-11-02T00:00:00+08:00");
}

#[test]
fn caldav_xml_yields_each_embedded_calendar_without_entity_expansion() {
    let xml = r#"<multistatus><response><calendar-data><![CDATA[BEGIN:VCALENDAR
BEGIN:VEVENT
UID:one
SUMMARY:One <b>bold</b>
DTSTART:20241101T090000Z
DTEND:20241101T100000Z
END:VEVENT
END:VCALENDAR]]></calendar-data></response></multistatus>"#;
    let calendars = calendar_documents(xml).unwrap();
    assert_eq!(calendars.len(), 1);
    assert!(calendars[0].contains("UID:one"));
    assert!(calendars[0].contains("One <b>bold</b>"));
    assert!(calendar_documents("<!DOCTYPE x [<!ENTITY y SYSTEM 'file:///etc/passwd'>]>").is_err());
}

#[test]
fn caldav_text_entities_and_failed_multistatus_are_distinguished_from_empty() {
    let xml="<d:multistatus><d:response><c:calendar-data>BEGIN:VCALENDAR\nSUMMARY:A &amp; B &#x1F600;\nEND:VCALENDAR</c:calendar-data></d:response></d:multistatus>";
    assert!(calendar_documents(xml).unwrap()[0].contains("A & B 😀"));
    assert!(calendar_documents("<d:multistatus><d:response><d:status>HTTP/1.1 403 Forbidden</d:status></d:response></d:multistatus>").is_err());
    assert!(calendar_documents("<d:multistatus/>").unwrap().is_empty());
}

#[tokio::test]
async fn root_service_discovers_collection_then_queries_it() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let replies=[
            ("PROPFIND / ",r#"<d:multistatus><d:response><d:current-user-principal><d:href>/principal/</d:href></d:current-user-principal></d:response></d:multistatus>"#),
            ("PROPFIND /principal/ ",r#"<d:multistatus><d:response><c:calendar-home-set><d:href>/home/</d:href></c:calendar-home-set></d:response></d:multistatus>"#),
            ("PROPFIND /home/ ",r#"<d:multistatus><d:response><d:href>/home/cal/</d:href><d:resourcetype><c:calendar/></d:resourcetype></d:response></d:multistatus>"#),
            ("REPORT /home/cal/ ","<d:multistatus><d:response><c:calendar-data><![CDATA[BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:fixture\nDTSTART:20261119T090000Z\nDTEND:20261119T100000Z\nSUMMARY:Fixture\nEND:VEVENT\nEND:VCALENDAR]]></c:calendar-data></d:response></d:multistatus>"),
        ];
        for (expected, body) in replies {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = stream.read(&mut buf).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
            }
            let text = String::from_utf8_lossy(&request);
            assert!(text.starts_with(expected), "unexpected method/path");
            assert!(text.to_ascii_lowercase().contains("authorization: basic "));
            stream.write_all(format!("HTTP/1.1 207 Multi-Status\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
        }
    });
    std::env::set_var("JEEVES_TEST_CALDAV_DISCOVERY_PASSWORD", "fixture-secret");
    let home = tempfile::tempdir().unwrap();
    let config = CalendarConfig {
        url,
        username: Some("fixture".into()),
        password: Some("env:JEEVES_TEST_CALDAV_DISCOVERY_PASSWORD".into()),
        timezone: "UTC".into(),
    };
    let docs = fetch_documents(
        home.path(),
        &config,
        "2026-11-19T00:00:00Z".parse().unwrap(),
        "2026-11-20T00:00:00Z".parse().unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(docs.len(), 1);
    assert!(docs[0].contains("UID:fixture"));
    server.await.unwrap();
}

#[test]
fn instant_at_week_boundary_belongs_only_to_the_new_week() {
    let ics = "BEGIN:VCALENDAR\nBEGIN:VEVENT\nUID:at-midnight\nDTSTART:20241103T160000Z\nEND:VEVENT\nEND:VCALENDAR";
    let slot = "2024-11-03T16:00:00Z".parse().unwrap();
    let week = chrono::Duration::weeks(1);
    assert!(events_in_range(ics, slot - week, slot, "Asia/Shanghai")
        .unwrap()
        .is_empty());
    assert_eq!(
        events_in_range(ics, slot, slot + week, "Asia/Shanghai")
            .unwrap()
            .len(),
        1
    );
}
