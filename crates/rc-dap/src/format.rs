//! Showing integer values in hexadecimal on the client, for adapters that do not honour the
//! DAP `ValueFormat { hex }` (GDB does: it prints with `output-radix 16`).

/// Width in bits and signedness of an integer type name (Rust and C spellings), or `None`
/// when the name is not a plain integer type.
fn integer_type(name: &str) -> Option<(u32, bool)> {
    let name = name.trim();
    Some(match name {
        "i8" | "signed char" | "int8_t" => (8, true),
        "u8" | "unsigned char" | "uint8_t" => (8, false),
        "i16" | "short" | "int16_t" => (16, true),
        "u16" | "unsigned short" | "uint16_t" => (16, false),
        "i32" | "int" | "int32_t" => (32, true),
        "u32" | "uint" | "unsigned int" | "unsigned" | "uint32_t" => (32, false),
        "i64" | "long" | "long long" | "int64_t" => (64, true),
        "u64" | "unsigned long" | "unsigned long long" | "uint64_t" => (64, false),
        "i128" => (128, true),
        "u128" => (128, false),
        "isize" | "ssize_t" => (usize::BITS, true),
        "usize" | "size_t" => (usize::BITS, false),
        _ => return None,
    })
}

/// `value` written in hexadecimal when it is a plain decimal integer (`255`, `-1`, `7u8`);
/// `None` for anything else (floats, strings, structs, chars, values already in hex).
///
/// A negative value needs its width, from `type_name` or the literal's suffix: it is shown in
/// two's complement (`-1` as `i8` is `0xff`). Without a known width it stays decimal. When
/// the type is known but is not an integer type (a float printed as `2`), nothing changes.
pub fn hex_integer(value: &str, type_name: Option<&str>) -> Option<String> {
    let text = value.trim();
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text),
    };
    let end = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    let (number, suffix) = digits.split_at(end);
    if number.is_empty() {
        return None;
    }
    // A leading zero followed by more digits could be octal or a non-decimal print.
    if number.len() > 1 && number.starts_with('0') {
        return None;
    }
    let suffix_type = if suffix.is_empty() { None } else { Some(integer_type(suffix)?) };
    let declared = type_name.filter(|t| !t.trim().is_empty());
    let width = match (declared, suffix_type) {
        (Some(t), _) => Some(integer_type(t)?),
        (None, s) => s,
    };
    let magnitude: u128 = number.parse().ok()?;
    if !negative {
        if let Some((bits, signed)) = width {
            let max = if signed { (1u128 << (bits - 1)) - 1 } else if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 };
            if magnitude > max {
                return None;
            }
        }
        return Some(format!("{magnitude:#x}"));
    }
    let (bits, signed) = width?;
    if !signed || magnitude == 0 || magnitude > 1u128 << (bits - 1) {
        return None;
    }
    let twos = magnitude.wrapping_neg();
    let mask = if bits == 128 { u128::MAX } else { (1u128 << bits) - 1 };
    Some(format!("{:#x}", twos & mask))
}

#[cfg(test)]
mod tests {
    use super::hex_integer;

    #[test]
    fn plain_integers_become_hex() {
        assert_eq!(hex_integer("255", Some("u32")).as_deref(), Some("0xff"));
        assert_eq!(hex_integer("0", Some("i32")).as_deref(), Some("0x0"));
        assert_eq!(hex_integer("10", None).as_deref(), Some("0xa"));
        assert_eq!(hex_integer("4096", Some("int")).as_deref(), Some("0x1000"));
        assert_eq!(hex_integer("340282366920938463463374607431768211455", Some("u128")).as_deref(), Some("0xffffffffffffffffffffffffffffffff"));
    }

    #[test]
    fn negative_integers_use_the_width_of_their_type() {
        assert_eq!(hex_integer("-1", Some("i8")).as_deref(), Some("0xff"));
        assert_eq!(hex_integer("-128", Some("i8")).as_deref(), Some("0x80"));
        assert_eq!(hex_integer("-1", Some("i32")).as_deref(), Some("0xffffffff"));
        assert_eq!(hex_integer("-2", Some("long")).as_deref(), Some("0xfffffffffffffffe"));
        assert_eq!(hex_integer("-5", Some("i128")).as_deref(), Some("0xfffffffffffffffffffffffffffffffb"));
        assert_eq!(hex_integer("-1i16", None).as_deref(), Some("0xffff"));
        // No width, out of range or unsigned: left alone.
        assert_eq!(hex_integer("-1", None), None);
        assert_eq!(hex_integer("-129", Some("i8")), None);
        assert_eq!(hex_integer("-1", Some("u8")), None);
        assert_eq!(hex_integer("256", Some("u8")), None);
    }

    #[test]
    fn suffixes_are_read() {
        assert_eq!(hex_integer("7u8", None).as_deref(), Some("0x7"));
        assert_eq!(hex_integer("300usize", None).as_deref(), Some("0x12c"));
        assert_eq!(hex_integer("7xyz", None), None);
    }

    #[test]
    fn everything_else_is_left_alone() {
        for v in ["2.5", "2", "\"12\"", "0x1f", "97 'a'", "'a'", "true", "{...}", "Some(3)", "", "-", "1e5", "007", "99999999999999999999999999999999999999999999"] {
            let ty = if v == "2" { Some("f64") } else { None };
            assert_eq!(hex_integer(v, ty), None, "{v}");
        }
        assert_eq!(hex_integer("5", Some("char")), None);
        assert_eq!(hex_integer("1", Some("bool")), None);
    }
}
