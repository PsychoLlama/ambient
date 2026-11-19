//! Moments, dates, and their text. See `docs/spec/time.md`.

#![allow(clippy::unwrap_used)]

mod support;

use support::text::text;

/// What `expr`, a string, gives with `ambient.time` imported.
fn time(expr: &str) -> String {
    text(&format!("use ambient.time;\nfn main(): string => {expr};"))
}

/// A moment written, then read back and written again.
fn written(moment: i64) -> (String, String) {
    let again = format!(
        "match time.read(time.write({moment})) {{ null => \"none\", let read: int => time.write(read) }}"
    );
    (time(&format!("time.write({moment})")), time(&again))
}

/// The moment some text says, written, or `none`.
fn read(source: &str) -> String {
    time(&format!(
        "match time.read(\"{source}\") {{ null => \"none\", let read: int => time.write(read) }}"
    ))
}

#[test]
fn a_moment_is_written_as_its_date_in_utc() {
    let cases = [
        (0, "1970-01-01T00:00:00.000Z"),
        (1_794_648_600_123, "2026-11-14T09:30:00.123Z"),
        // The day a leap year adds, and the day before 1970.
        (951_782_400_000, "2000-02-29T00:00:00.000Z"),
        (-1, "1969-12-31T23:59:59.999Z"),
        (-2_208_988_800_000, "1900-01-01T00:00:00.000Z"),
    ];
    for (moment, expected) in cases {
        let same = (expected.to_owned(), expected.to_owned());
        assert_eq!(written(moment), same);
    }
}

#[test]
fn text_is_read_at_any_offset_and_to_any_precision() {
    assert_eq!(read("2026-11-14T09:30:00Z"), "2026-11-14T09:30:00.000Z");
    assert_eq!(
        read("2026-11-14T10:30:00.5+01:00"),
        "2026-11-14T09:30:00.500Z"
    );
    assert_eq!(
        read("2026-11-13 23:30:00.123456-10:00"),
        "2026-11-14T09:30:00.123Z"
    );
    assert_eq!(read("2026-11-14T09:30"), "2026-11-14T09:30:00.000Z");
    assert_eq!(read("2026-11-14"), "2026-11-14T00:00:00.000Z");
}

#[test]
fn text_that_says_no_moment_is_none() {
    let none = [
        "2026-02-30",
        "2026-13-01",
        "2026-11-14T24:00:00Z",
        "2026-11-14T09:30:00+0100",
        "2026-11-14T09:30:00.Z",
        "2026-11-14T09:30:00 and more",
        "14/11/2026",
        "",
    ];
    for source in none {
        assert_eq!(read(source), "none", "{source}");
    }
}

#[test]
fn a_date_that_does_not_exist_has_no_moment() {
    let moment = |day: i64| {
        let date = format!(
            "{{ year: 2026, month: 2, day: {day}, hour: 0, minute: 0, second: 0, millisecond: 0 }}"
        );
        time(&format!(
            "match time.moment({date}) {{ null => \"none\", let at: int => at.to_text() }}"
        ))
    };
    assert_eq!(
        (moment(28), moment(29)),
        ("1772236800000".into(), "none".into())
    );
}

#[test]
fn a_moment_has_a_day_of_the_week_and_lengths_add_to_it() {
    // The first day of 1970 was a Thursday; ten days on is a Sunday.
    assert_eq!(time("time.weekday(0).to_text()"), "4");
    assert_eq!(time("time.weekday(time.days(10)).to_text()"), "7");
    assert_eq!(time("time.weekday(0 - time.days(3)).to_text()"), "1");
    let later = "time.write(time.days(1) + time.hours(2) + time.minutes(3) + time.seconds(4))";
    assert_eq!(time(later), "1970-01-02T02:03:04.000Z");
}
