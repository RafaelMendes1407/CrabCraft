//! Bounded RFC4180-style UTF-8 CSV. Explicit NULL/hex markers; no SQL or formulas execute.
use crate::database_core::{Cell, Error, Transfer};
fn formula(value: &str) -> bool {
    value.trim_start_matches([' ', '\t', '\r', '\n']).starts_with(['=', '+', '-', '@'])
}
fn text(value: &str) -> Result<String, Error> {
    if value.chars().any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t')) {
        return Err(Error::Input);
    }
    Ok(if value.starts_with('\\') || formula(value) { format!("\\{value}") } else { value.into() })
}
fn field(value: &str, out: &mut String) {
    if value.contains([',', '"', '\n', '\r']) {
        out.push('"');
        out.push_str(&value.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(value);
    }
}
pub fn encode(transfer: &Transfer) -> Result<Vec<u8>, Error> {
    if transfer.rows.len() > 5000 || transfer.columns.is_empty() || transfer.columns.len() > 256 {
        return Err(Error::Limit);
    }
    let mut out = String::new();
    for (index, name) in transfer.columns.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        field(&text(name)?, &mut out);
    }
    out.push_str("\r\n");
    for row in &transfer.rows {
        if row.len() != transfer.columns.len() {
            return Err(Error::Input);
        }
        for (index, cell) in row.iter().enumerate() {
            if index > 0 {
                out.push(',');
            }
            let value = match cell {
                Cell::Null => "\\N".into(),
                Cell::Blob(value) => format!("\\x{}", value.iter().map(|byte| format!("{byte:02x}")).collect::<String>()),
                Cell::Text(value) => text(value)?,
                value => value.display(),
            };
            field(&value, &mut out);
            if out.len() > 8 * 1024 * 1024 {
                return Err(Error::Limit);
            }
        }
        out.push_str("\r\n");
    }
    Ok(out.into_bytes())
}
fn unescape(value: &str) -> Option<&str> {
    value.strip_prefix('\\').filter(|rest| rest.starts_with('\\') || formula(rest))
}
fn value(field: String) -> Result<Cell, Error> {
    if let Some(text) = unescape(&field) {
        return Ok(Cell::Text(text.into()));
    }
    if field == "\\N" {
        return Ok(Cell::Null);
    }
    if let Some(hex) = field.strip_prefix("\\x") {
        if hex.len() % 2 != 0 || !hex.is_ascii() {
            return Err(Error::Input);
        }
        let bytes = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| std::str::from_utf8(pair).ok().and_then(|text| u8::from_str_radix(text, 16).ok()).ok_or(Error::Input))
            .collect::<Result<Vec<_>, _>>()?;
        return Ok(Cell::Blob(bytes));
    }
    Ok(Cell::Text(field))
}
pub fn decode(bytes: &[u8]) -> Result<Transfer, Error> {
    if bytes.len() > 8 * 1024 * 1024 {
        return Err(Error::Limit);
    }
    let input = std::str::from_utf8(bytes).map_err(|_| Error::Input)?.strip_prefix('\u{feff}').unwrap_or(std::str::from_utf8(bytes).map_err(|_| Error::Input)?);
    if input.contains('\0') {
        return Err(Error::Input);
    }
    let mut records = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut quote = false;
    let mut closed = false;
    let mut at_start = true;
    let mut chars = input.chars().peekable();
    let mut ended = true;
    while let Some(ch) = chars.next() {
        ended = false;
        if quote {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quote = false;
                    closed = true;
                }
            } else {
                field.push(ch);
            }
        } else if ch == ',' {
            row.push(std::mem::take(&mut field));
            closed = false;
            at_start = true;
        } else if ch == '\n' || ch == '\r' {
            if ch == '\r' && chars.peek() == Some(&'\n') {
                chars.next();
            }
            row.push(std::mem::take(&mut field));
            if row.len() > 256 {
                return Err(Error::Limit);
            }
            records.push(std::mem::take(&mut row));
            closed = false;
            at_start = true;
            ended = true;
            if records.len() > 5001 {
                return Err(Error::Limit);
            }
        } else if ch == '"' && at_start {
            quote = true;
            at_start = false;
        } else {
            if closed || ch == '"' {
                return Err(Error::Input);
            }
            field.push(ch);
            at_start = false;
        }
        if field.len() > 1024 * 1024 || row.len() > 256 {
            return Err(Error::Limit);
        }
    }
    if quote {
        return Err(Error::Input);
    }
    if !ended {
        row.push(field);
        records.push(row);
    }
    if records.is_empty() || records.len() > 5001 {
        return Err(Error::Input);
    }
    let columns: Vec<String> = records.remove(0).into_iter().map(|value| unescape(&value).unwrap_or(&value).to_string()).collect();
    if columns.is_empty() || columns.len() > 256 || columns.iter().any(|name| name.len() > 1024) {
        return Err(Error::Input);
    }
    let mut bytes = 0_usize;
    let mut rows = Vec::new();
    for row in records {
        if row.len() != columns.len() {
            return Err(Error::Input);
        }
        let row = row.into_iter().map(value).collect::<Result<Vec<_>, _>>()?;
        bytes += row.iter().map(Cell::size).sum::<usize>();
        if bytes > 2 * 1024 * 1024 {
            return Err(Error::Limit);
        }
        rows.push(row);
    }
    Ok(Transfer { format: "crabcraft.csv.rows.v1".into(), columns, rows })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_quotes_null_empty_blob_unicode_and_formula_cells_round_trip_safely() {
        let original = Transfer {
            format: "ignored".into(),
            columns: vec!["name".into(), "value".into()],
            rows: vec![
                vec![Cell::Text("comma, quote\" and newline\n🦀".into()), Cell::Null],
                vec![Cell::Text(String::new()), Cell::Blob(vec![0, 1, 255])],
                vec![Cell::Text("\\N".into()), Cell::Text("=WEBSERVICE(\"example\")".into())],
                vec![Cell::Text("\\x00ff".into()), Cell::Text(" \t+formula".into())],
            ],
        };
        let bytes = encode(&original).unwrap();
        assert!(std::str::from_utf8(&bytes).unwrap().contains("\\=WEBSERVICE"));
        let parsed = decode(&bytes).unwrap();
        assert_eq!(parsed.columns, original.columns);
        assert_eq!(parsed.rows, original.rows);
        assert!(decode(b"a,b\r\n\"unterminated,x").is_err());
        assert!(decode(b"a,b\r\nx\"y,z").is_err());
    }
}
