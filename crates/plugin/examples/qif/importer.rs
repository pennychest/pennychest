//! Reads QIF (Quicken Interchange Format) bank statements, which many banks still offer as
//! a download:
//!
//! ```text
//! !Type:Bank
//! D20/09/2026
//! T-12.50
//! PTESCO STORES 2041
//! ^
//! ```
//!
//! Each record is a field per line, the first letter saying which (D date, T amount, P payee,
//! M memo), ending with `^`.

use pennychest_plugin::{Decimal, Importer, ImporterInfo, NaiveDate, Row};

pub struct Qif;

/// The formats banks write QIF dates in, day first as UK banks do.
// "%y" first: "%Y" would read the "26" of "20/09/26" as the year 26.
const DATE_FORMATS: [&str; 4] = ["%d/%m/%y", "%d/%m/%Y", "%Y-%m-%d", "%d-%m-%Y"];

fn date(text: &str) -> Option<NaiveDate> {
    // Quicken writes two-digit years after 2000 with an apostrophe: 20/09'26.
    let text = text.trim().replace('\'', "/");
    DATE_FORMATS
        .iter()
        .find_map(|format| NaiveDate::parse_from_str(&text, format).ok())
}

/// One record's fields, as they're read.
#[derive(Default)]
struct Record {
    first_line: usize,
    lines: Vec<String>,
    date: Option<String>,
    amount: Option<String>,
    payee: Option<String>,
    memo: Option<String>,
}

impl Record {
    fn row(self) -> Result<Row, String> {
        let at = self.first_line;
        let date_text = self
            .date
            .ok_or_else(|| format!("Line {at}: the transaction has no date (a D line)."))?;
        let date = date(&date_text).ok_or_else(|| {
            format!("Line {at}: \"{date_text}\" isn't a date this importer reads.")
        })?;
        let amount_text = self
            .amount
            .ok_or_else(|| format!("Line {at}: the transaction has no amount (a T line)."))?;
        let amount: Decimal = amount_text
            .replace(',', "")
            .trim()
            .parse()
            .map_err(|_| format!("Line {at}: \"{amount_text}\" isn't an amount."))?;
        let description = self
            .payee
            .or(self.memo)
            .unwrap_or_default()
            .trim()
            .to_owned();
        Ok(Row {
            date,
            description,
            amount,
            raw_content: self.lines.join("\n"),
            line_number: i64::try_from(at).ok(),
            metadata: Default::default(),
        })
    }
}

impl Importer for Qif {
    fn info(&self) -> ImporterInfo {
        ImporterInfo::new("qif", "QIF statements", &["qif"])
            .describe("Quicken Interchange Format bank and card statements")
            .detects()
    }

    /// QIF files start with a `!Type:` header, whatever they're called.
    fn detect(&self, file: &[u8], _filename: &str) -> bool {
        let start = String::from_utf8_lossy(&file[..file.len().min(64)]);
        start
            .trim_start_matches('\u{feff}')
            .trim_start()
            .starts_with("!Type:")
    }

    fn parse(&self, file: &[u8]) -> Result<Vec<Row>, String> {
        let text = String::from_utf8_lossy(file);
        let mut rows = Vec::new();
        let mut record = Record::default();
        for (index, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            let Some(field) = line.chars().next() else {
                continue;
            };
            if field == '!' {
                continue;
            }
            if field == '^' {
                if !record.lines.is_empty() {
                    rows.push(std::mem::take(&mut record).row()?);
                }
                continue;
            }
            if record.lines.is_empty() {
                record.first_line = index + 1;
            }
            record.lines.push(line.to_owned());
            let value = line[field.len_utf8()..].to_owned();
            match field {
                'D' => record.date = Some(value),
                'T' | 'U' => record.amount = Some(value),
                'P' => record.payee = Some(value),
                'M' => record.memo = Some(value),
                _ => {}
            }
        }
        if !record.lines.is_empty() {
            rows.push(record.row()?);
        }
        if rows.is_empty() {
            return Err("The file has no transactions.".into());
        }
        Ok(rows)
    }
}
