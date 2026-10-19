//! Reading what the phone's wallet shortcut sends, which is whatever the wallet trigger
//! provides: amounts like "£12.50" or "12,50 €", and dates in the phone's own format.

use std::sync::LazyLock;

use chrono::{DateTime, Datelike, NaiveDate, NaiveDateTime};
use regex::Regex;
use rust_decimal::Decimal;
use serde_json::Value;

const CURRENCY_SYMBOLS: [(char, &str); 3] = [('£', "GBP"), ('€', "EUR"), ('$', "USD")];

const DATE_FORMATS: [&str; 7] = [
    "%d %b %Y",
    "%d %B %Y",
    "%d/%m/%Y",
    "%d/%m/%y",
    "%Y/%m/%d",
    "%b %d, %Y",
    "%B %d, %Y",
];

/// A string in single quotes (or double, if it holds a single quote), for messages that quote
/// what couldn't be read.
fn quoted(text: &str) -> String {
    if text.contains('\'') && !text.contains('"') {
        format!("\"{text}\"")
    } else {
        format!("'{}'", text.replace('\'', "\\'"))
    }
}

static CURRENCY_CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([A-Z]{3})\b").expect("a valid pattern"));

/// An amount such as "£12.50", "12,50 €", "-3.00" or 12.5, and its currency if it says.
pub fn amount(value: &Value) -> Result<(Decimal, Option<String>), String> {
    let text = match value {
        Value::Number(number) => {
            let number = number.to_string();
            return number
                .parse::<Decimal>()
                .or_else(|_| Decimal::from_scientific(&number))
                .map(|d| (d, None))
                .map_err(|_| format!("could not parse amount {number}"));
        }
        Value::String(text) if !text.trim().is_empty() => text.trim(),
        _ => return Err("amount is required".into()),
    };
    let currency = CURRENCY_SYMBOLS
        .iter()
        .find(|(symbol, _)| text.contains(*symbol))
        .map(|(_, code)| (*code).to_owned())
        .or_else(|| Some(CURRENCY_CODE.captures(text)?.get(1)?.as_str().to_owned()));

    let mut number: String = text
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-'))
        .collect();
    match (number.rfind(','), number.rfind('.')) {
        // Whichever separator comes last is the decimal point.
        (Some(comma), Some(point)) if comma > point => {
            number = number.replace('.', "").replace(',', ".");
        }
        (Some(_), Some(_)) => number = number.replace(',', ""),
        // "12,50" is a decimal comma; "1,250" is a thousands separator.
        (Some(comma), None) => {
            let (head, tail) = (&number[..comma], &number[comma + 1..]);
            number = if tail.len() == 2 {
                format!("{}.{tail}", head.replace(',', ""))
            } else {
                number.replace(',', "")
            };
        }
        _ => {}
    }
    let parsed = number
        .parse::<Decimal>()
        .map_err(|_| format!("could not parse amount {}", quoted(text)))?;
    Ok((parsed, currency))
}

static TIME_PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s+at\s+|,\s*\d{1,2}:\d{2}|\s+\d{1,2}:\d{2}").expect("a valid pattern")
});

/// An ISO 8601 date or date and time; the date
/// is the one written, whatever the offset.
fn iso(text: &str) -> Option<NaiveDate> {
    let text = text.replace('Z', "+00:00");
    if let Ok(date) = NaiveDate::parse_from_str(&text, "%Y-%m-%d") {
        return Some(date);
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f%:z",
        "%Y-%m-%d %H:%M:%S%.f%:z",
        "%Y-%m-%dT%H:%M%:z",
    ] {
        if let Ok(at) = DateTime::parse_from_str(&text, format) {
            return Some(at.date_naive());
        }
    }
    for format in [
        "%Y-%m-%dT%H:%M:%S%.f",
        "%Y-%m-%d %H:%M:%S%.f",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(at) = NaiveDateTime::parse_from_str(&text, format) {
            return Some(at.date());
        }
    }
    None
}

/// The tap's date, `today` when none is sent.
pub fn date(value: &Value, today: NaiveDate) -> Result<NaiveDate, String> {
    let text = match value {
        Value::Null => return Ok(today),
        Value::String(text) if text.trim().is_empty() => return Ok(today),
        Value::String(text) => text.trim(),
        other => return Err(format!("could not parse date {other}")),
    };
    if let Some(date) = iso(text) {
        return Ok(date);
    }
    // Drop any time part, e.g. "26 Sep 2026 at 17:02".
    let date_part = TIME_PART.split(text).next().unwrap_or_default().trim();
    for format in DATE_FORMATS {
        if let Ok(date) = NaiveDate::parse_from_str(date_part, format) {
            // "%Y" also reads a two-digit year, which "%y" is for.
            if !format.contains("%Y") || date.year() >= 1000 {
                return Ok(date);
            }
        }
    }
    Err(format!("could not parse date {}", quoted(text)))
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn decimal(text: &str) -> Decimal {
        text.parse().unwrap()
    }

    #[test]
    fn amounts_in_the_wallets_formats() {
        let cases = [
            (json!("£12.50"), "12.50", Some("GBP")),
            (json!("12,50 €"), "12.50", Some("EUR")),
            (json!("$1,250.00"), "1250.00", Some("USD")),
            (json!("1.250,75 EUR"), "1250.75", Some("EUR")),
            (json!("-3.00"), "-3.00", None),
            (json!("1,250"), "1250", None),
            (json!(7.5), "7.5", None),
            (json!(12), "12", None),
        ];
        for (value, expected, currency) in cases {
            let currency = currency.map(str::to_owned);
            assert_eq!(amount(&value), Ok((decimal(expected), currency)), "{value}");
        }
        assert_eq!(
            amount(&json!("abc")),
            Err("could not parse amount 'abc'".into())
        );
        assert_eq!(amount(&json!(" ")), Err("amount is required".into()));
        assert_eq!(amount(&Value::Null), Err("amount is required".into()));
    }

    #[test]
    fn dates_in_the_phones_formats() {
        let today = NaiveDate::from_ymd_opt(2026, 10, 14).unwrap();
        let expected = NaiveDate::from_ymd_opt(2026, 9, 26).unwrap();
        for text in [
            "2026-09-26",
            "2026-09-26T17:02:13+01:00",
            "2026-09-26T23:30:00Z",
            "2026-09-26 17:02",
            "26 Sep 2026 at 17:02",
            "26 September 2026",
            "26/09/2026",
            "26/09/26",
            "2026/09/26",
            "Sep 26, 2026",
            "September 26, 2026, 17:02",
            "26 Sep 2026 17:02",
        ] {
            assert_eq!(date(&json!(text), today), Ok(expected), "{text}");
        }
        assert_eq!(date(&json!(""), today), Ok(today));
        assert_eq!(date(&Value::Null, today), Ok(today));
        assert_eq!(
            date(&json!("not a date"), today),
            Err("could not parse date 'not a date'".into())
        );
    }
}
