//! The built-in CSV importer. It recognises common UK bank statement layouts by their column
//! names; other layouts are imported with a column mapping, chosen by the user, suggested by a
//! model, or saved as a template.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use rust_decimal::Decimal;

use super::{ImportedRow, Importer, handles_extension};
use crate::Money;

pub const NAME: &str = "csv";

const DATE_COLUMNS: &[&str] = &[
    "date",
    "transaction date",
    "trans date",
    "value date",
    "posted date",
    "booking date",
];
const DESCRIPTION_COLUMNS: &[&str] = &[
    "description",
    "narrative",
    "details",
    "transaction description",
    "memo",
    "reference",
    "payee",
    "name",
];
const AMOUNT_COLUMNS: &[&str] = &["amount", "value", "transaction amount"];
const DEBIT_COLUMNS: &[&str] = &[
    "debit",
    "debit amount",
    "money out",
    "paid out",
    "withdrawals",
];
const CREDIT_COLUMNS: &[&str] = &["credit", "credit amount", "money in", "paid in", "deposits"];

/// Tried in this order, so day-first wins over month-first.
const DATE_FORMATS: &[&str] = &[
    "%d/%m/%Y", "%Y-%m-%d", "%d-%m-%Y", "%d/%m/%y", "%d-%m-%y", "%Y/%m/%d", "%m/%d/%Y", "%d %b %Y",
    "%d %B %Y",
];

/// What a column can hold, and how each is named to the user, in the order they're shown.
pub const FIELDS: &[(&str, &str)] = &[
    ("date", "date"),
    ("description", "description"),
    ("amount", "amount"),
    ("money_out", "money out"),
    ("money_in", "money in"),
];

/// Which column (counting from 0) holds each field.
pub type Columns = BTreeMap<String, usize>;

pub fn normalise_header(header: &str) -> String {
    header.trim().to_lowercase().replace('_', " ")
}

fn find_column(headers: &[String], candidates: &[&str]) -> Option<usize> {
    headers
        .iter()
        .position(|h| candidates.contains(&normalise_header(h).as_str()))
}

/// The header row and the data rows. A UTF-8 byte order mark is ignored.
pub fn read(content: &[u8]) -> Result<(Vec<String>, Vec<Vec<String>>), String> {
    let text = std::str::from_utf8(content).map_err(|_| "the file isn't UTF-8 text".to_owned())?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .flexible(true)
        .from_reader(text.as_bytes());
    let mut rows = Vec::new();
    for record in reader.records() {
        let record = record.map_err(|e| e.to_string())?;
        rows.push(record.iter().map(str::to_owned).collect::<Vec<_>>());
    }
    let mut rows = rows.into_iter();
    let headers = rows.next().filter(|h| !(h.len() == 1 && h[0].is_empty()));
    let Some(headers) = headers.filter(|h| !h.is_empty()) else {
        return Err("CSV file is empty or has no header row".into());
    };
    Ok((headers, rows.collect()))
}

/// The columns whose header names a field, e.g. "Paid out" for money out.
pub fn columns_by_name(headers: &[String]) -> Columns {
    [
        ("date", DATE_COLUMNS),
        ("description", DESCRIPTION_COLUMNS),
        ("amount", AMOUNT_COLUMNS),
        ("money_out", DEBIT_COLUMNS),
        ("money_in", CREDIT_COLUMNS),
    ]
    .into_iter()
    .filter_map(|(field, names)| find_column(headers, names).map(|c| (field.to_owned(), c)))
    .collect()
}

/// The fields a mapping still needs, as the user knows them: a date, a description, and some
/// amount.
pub fn missing_fields(columns: &Columns) -> Vec<&'static str> {
    let mut missing: Vec<&'static str> = ["date", "description"]
        .into_iter()
        .filter(|f| !columns.contains_key(*f))
        .collect();
    if ["amount", "money_out", "money_in"]
        .iter()
        .all(|f| !columns.contains_key(*f))
    {
        missing.push("amount");
    }
    missing
}

fn parse_date(value: &str) -> Option<NaiveDate> {
    let value = value.trim();
    DATE_FORMATS
        .iter()
        .find_map(|format| NaiveDate::parse_from_str(value, format).ok())
}

/// An amount as banks write them: thousands separators and currency symbols are ignored, and
/// an empty cell or "-" is zero.
fn parse_amount(value: &str) -> Option<Money> {
    let cleaned: String = value
        .trim()
        .chars()
        .filter(|c| !matches!(c, ',' | '£' | '$' | '€'))
        .collect();
    if cleaned.is_empty() || cleaned == "-" {
        return Some(Money::ZERO);
    }
    let decimal = Decimal::from_str_exact(&cleaned)
        .or_else(|_| Decimal::from_scientific(&cleaned))
        .ok()?;
    Money::from_decimal(decimal.round_dp(crate::money::SCALE)).ok()
}

/// Reads rows with `columns` saying which column holds each field: "date", "description",
/// and "amount" or one or both of "money_out" and "money_in". Rows without a readable date,
/// description or amount, or with a zero amount, are skipped.
pub fn parse_mapped(content: &[u8], columns: &Columns) -> Result<Vec<ImportedRow>, String> {
    let missing = missing_fields(columns);
    if !missing.is_empty() {
        return Err(format!(
            "Choose the columns for the {}.",
            missing.join(" and ")
        ));
    }
    let (_, data) = read(content)?;
    let date_col = columns["date"];
    let description_col = columns["description"];
    let amount_col = columns.get("amount");
    let out_col = columns.get("money_out");
    let in_col = columns.get("money_in");

    let mut rows = Vec::new();
    // The header is line 1
    for (row, line_number) in data.iter().zip(2_i64..) {
        if row.iter().all(|cell| cell.trim().is_empty()) {
            continue;
        }
        let Some(date) = row.get(date_col).and_then(|v| parse_date(v)) else {
            continue;
        };
        let Some(description) = row.get(description_col).map(|d| d.trim()) else {
            continue;
        };
        if description.is_empty() {
            continue;
        }
        let cell = |column: Option<&usize>| -> Option<Money> {
            match column {
                Some(&c) => row.get(c).and_then(|v| parse_amount(v)),
                None => Some(Money::ZERO),
            }
        };
        let amount = if amount_col.is_some() {
            cell(amount_col)
        } else {
            // Money out is always outgoing, whether the bank writes 12.50 or -12.50.
            match (cell(out_col), cell(in_col)) {
                (Some(out), Some(r#in)) => Some(r#in - out.abs()),
                _ => None,
            }
        };
        let Some(amount) = amount.filter(|a| !a.is_zero()) else {
            continue;
        };
        rows.push(ImportedRow {
            date,
            description: description.to_owned(),
            amount,
            raw_content: row.join(","),
            line_number: Some(line_number),
            metadata: serde_json::Map::new(),
        });
    }
    Ok(rows)
}

pub struct CsvImporter;

impl Importer for CsvImporter {
    fn name(&self) -> &str {
        NAME
    }

    fn label(&self) -> &str {
        "CSV"
    }

    fn description(&self) -> &str {
        "Generic CSV bank statement importer"
    }

    fn file_types(&self) -> Vec<String> {
        vec!["csv".into(), "txt".into()]
    }

    /// A CSV file whose headers name a date, a description and an amount (or money out).
    fn detect(&self, content: &[u8], filename: &str) -> bool {
        if !handles_extension(&self.file_types(), filename) {
            return false;
        }
        let Ok((headers, _)) = read(content) else {
            return false;
        };
        let names: Vec<String> = headers.iter().map(|h| normalise_header(h)).collect();
        let has = |set: &[&str]| names.iter().any(|n| set.contains(&n.as_str()));
        has(DATE_COLUMNS) && has(DESCRIPTION_COLUMNS) && (has(AMOUNT_COLUMNS) || has(DEBIT_COLUMNS))
    }

    fn parse(&self, content: &[u8]) -> Result<Vec<ImportedRow>, String> {
        let (headers, _) = read(content)?;
        let columns = columns_by_name(&headers);
        let missing = missing_fields(&columns);
        if !missing.is_empty() {
            return Err(format!(
                "Couldn't tell which columns hold the {}. Headers: {}.",
                missing.join(" and "),
                quoted_list(&headers)
            ));
        }
        parse_mapped(content, &columns)
    }
}

/// A list of strings in brackets, each quoted: `['Date', 'Who']`.
fn quoted_list(items: &[String]) -> String {
    let quoted: Vec<String> = items
        .iter()
        .map(|s| format!("'{}'", s.replace('\\', "\\\\").replace('\'', "\\'")))
        .collect();
    format!("[{}]", quoted.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    const ODD: &str = "Posted,Ref,Who,Out,In,Running total\n\
                       20/09/2026,A1,TESCO STORES,12.50,,987.50\n\
                       21/09/2026,A2,SALARY,,1500.00,2487.50\n";

    fn m(s: &str) -> Money {
        s.parse().unwrap()
    }

    fn columns(pairs: &[(&str, usize)]) -> Columns {
        pairs.iter().map(|(f, c)| ((*f).to_owned(), *c)).collect()
    }

    #[test]
    fn reads_known_layouts_by_their_column_names() {
        let content = "\u{feff}Date,Description,Amount\n\
                       20/09/2026,TESCO,-12.50\n\
                       21/09/2026,\"SMITH, J\",\"1,500.00\"\n\
                       ,,\n\
                       22/09/2026,ZERO,0\n\
                       bad date,X,1\n";
        assert!(CsvImporter.detect(content.as_bytes(), "s.csv"));
        let rows = CsvImporter.parse(content.as_bytes()).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].amount, m("-12.5"));
        assert_eq!(rows[0].date, NaiveDate::from_ymd_opt(2026, 9, 20).unwrap());
        assert_eq!(rows[0].line_number, Some(2));
        assert_eq!(rows[1].description, "SMITH, J");
        assert_eq!(rows[1].amount, m("1500"));
        assert_eq!(rows[1].raw_content, "21/09/2026,SMITH, J,1,500.00");
    }

    #[test]
    fn doesnt_recognise_odd_layouts_or_other_files() {
        assert!(!CsvImporter.detect(ODD.as_bytes(), "s.csv"));
        assert!(!CsvImporter.detect(b"Date,Description,Amount\n", "s.pdf"));
        let err = CsvImporter.parse(ODD.as_bytes()).unwrap_err();
        assert_eq!(
            err,
            "Couldn't tell which columns hold the date and description and amount. Headers: \
             ['Posted', 'Ref', 'Who', 'Out', 'In', 'Running total']."
        );
    }

    #[test]
    fn imports_with_a_mapping() {
        let mapping = columns(&[
            ("date", 0),
            ("description", 2),
            ("money_out", 3),
            ("money_in", 4),
        ]);
        let rows = parse_mapped(ODD.as_bytes(), &mapping).unwrap();
        let amounts: Vec<(&str, Money)> = rows
            .iter()
            .map(|r| (r.description.as_str(), r.amount))
            .collect();
        assert_eq!(
            amounts,
            [("TESCO STORES", m("-12.5")), ("SALARY", m("1500"))]
        );
    }

    #[test]
    fn money_out_is_outgoing_whichever_sign_the_bank_uses() {
        let mapping = columns(&[
            ("date", 0),
            ("description", 1),
            ("money_out", 2),
            ("money_in", 3),
        ]);
        let rows =
            parse_mapped(b"Posted,Who,Out,In\n20/09/2026,TESCO,-12.50,\n", &mapping).unwrap();
        assert_eq!(rows[0].amount, m("-12.5"));
    }

    #[test]
    fn a_mapping_must_be_complete() {
        let err = parse_mapped(ODD.as_bytes(), &columns(&[("date", 0)])).unwrap_err();
        assert_eq!(err, "Choose the columns for the description and amount.");
    }

    #[test]
    fn dates_and_amounts_in_the_forms_banks_write() {
        for (text, ymd) in [
            ("20/09/2026", (2026, 9, 20)),
            ("2026-09-20", (2026, 9, 20)),
            ("20-09-2026", (2026, 9, 20)),
            ("2026/09/20", (2026, 9, 20)),
            ("20 Sep 2026", (2026, 9, 20)),
            ("20 September 2026", (2026, 9, 20)),
            ("3/7/2026", (2026, 7, 3)),
        ] {
            let (y, mo, d) = ymd;
            assert_eq!(
                parse_date(text),
                NaiveDate::from_ymd_opt(y, mo, d),
                "{text}"
            );
        }
        assert_eq!(parse_amount("£1,234.56"), Some(m("1234.56")));
        assert_eq!(parse_amount(" -"), Some(Money::ZERO));
        assert_eq!(parse_amount("abc"), None);
    }

    #[test]
    fn empty_files_have_no_header() {
        assert_eq!(
            read(b"").unwrap_err(),
            "CSV file is empty or has no header row"
        );
        assert!(read(b"\xff\xfe").is_err());
    }
}
