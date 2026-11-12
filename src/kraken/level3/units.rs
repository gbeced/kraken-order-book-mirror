use rust_decimal::Decimal;
use std::error::Error;
use std::fmt::NumBuffer;
use std::num::IntErrorKind;

pub fn to_units(value: Decimal, scale: u8) -> Result<u64, Box<dyn Error>> {
    if value.is_sign_negative() {
        return Err(other_error("Negative value"));
    }
    let scale = u32::from(scale);
    let mantissa = value.mantissa();
    let value_scale = value.scale();
    let units = if value_scale <= scale {
        let factor = 10i128
            .checked_pow(scale - value_scale)
            .ok_or_else(overflow_error)?;
        mantissa.checked_mul(factor).ok_or_else(overflow_error)?
    } else {
        let factor = 10i128
            .checked_pow(value_scale - scale)
            .ok_or_else(overflow_error)?;
        if mantissa % factor != 0 {
            return Err(other_error("value is not a multiple of the increment"));
        }
        mantissa / factor
    };

    u64::try_from(units).map_err(|_| overflow_error())
}

/// Parses raw JSON number text, e.g. `83510.7` or `0.00005100`, into a count of `10^-scale` units.
/// Only positive decimals are accepted: no exponent, at least one digit on each side of a `.`.
pub fn parse_units(text: &str, scale: u8) -> Result<u64, Box<dyn Error>> {
    fn exp_scale(num: u64, exp: u32) -> Result<u64, Box<dyn Error>> {
        let res = num
            .checked_mul(10_u64.checked_pow(exp).ok_or_else(overflow_error)?)
            .ok_or_else(overflow_error)?;
        Ok(res)
    }

    // Split into two, and validate that each part is a valid number.
    let (int_part, frac_part) = match text.split_once('.') {
        Some((int_part, frac_part)) => (int_part, Some(frac_part)),
        None => (text, None),
    };

    // Parse and validate the integer part.
    let parsed_int = int_part.parse::<u64>().map_err(|e| match e.kind() {
        IntErrorKind::PosOverflow => overflow_error(),
        IntErrorKind::NegOverflow => overflow_error(),
        _ => other_error("Invalid number"),
    })?;
    let mut units: u64 = exp_scale(parsed_int, scale.into())?;

    // Parse and validate the fractional part.
    if let Some(frac_part) = frac_part {
        if frac_part.is_empty() {
            return Err(other_error("Invalid number"));
        }

        let mut exp: u32 = scale.into();
        let mut it = frac_part.bytes();
        while exp > 0
            && let Some(byte) = it.next()
        {
            exp -= 1;

            if byte == b'0' {
                continue;
            }
            let Some(digit) = char::from(byte).to_digit(10) else {
                return Err(other_error("Invalid number"));
            };

            let ddunits = u64::from(digit)
                .checked_mul(10_u64.checked_pow(exp).ok_or_else(overflow_error)?)
                .ok_or_else(overflow_error)?;
            units = units.checked_add(ddunits).ok_or_else(overflow_error)?;
        }

        // Check for trailing non-zero digits that exceed scale.
        for byte in it {
            if byte != b'0' {
                return Err(other_error("Value exceeds the maximum precision"));
            }
        }
    }

    Ok(units)
}

pub fn to_decimal(units: u64, scale: u8) -> Decimal {
    Decimal::from_i128_with_scale(units as i128, u32::from(scale))
}

pub fn serialize_units(units: u64, out: &mut String) {
    if units == 0 {
        return;
    }
    let mut buf = NumBuffer::new();
    out.push_str(units.format_into(&mut buf));
}

fn overflow_error() -> Box<dyn Error> {
    "Overflow".into()
}

fn other_error(msg: &str) -> Box<dyn Error> {
    msg.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn units_conversion_round_trip() {
        let cases = [
            ("0", 0 as u8, 0 as u64),
            ("0", 8, 0),
            ("0.00000000", 8, 0),
            ("1", 0, 1),
            ("+1", 0, 1),
            ("1", 8, 100000000),
            ("1.5", 8, 150000000),
            ("1.50", 1, 15),
            ("10.00", 1, 100),
            ("80796.0", 1, 807960),
            ("80796.1", 1, 807961),
            ("80796.1", 2, 8079610),
            ("75644.7", 1, 756447),
            ("75644.70", 1, 756447),
            ("0.00005100", 8, 5100),
            ("0.46450888", 8, 46450888),
            ("1.32196829", 8, 132196829),
            ("1.320", 2, 132),
        ];

        for (str_value, scale, units) in cases {
            // From decimal to units.
            assert_eq!(
                to_units(Decimal::from_str(str_value).unwrap(), scale).unwrap(),
                units
            );
            // From raw number text to units.
            assert_eq!(parse_units(str_value, scale).unwrap(), units, "{str_value}");
            // From units to decimal.
            assert_eq!(
                to_decimal(units, scale),
                Decimal::from_str(str_value).unwrap()
            );
        }
    }

    #[test]
    fn parse_units_edges() {
        let ok = [
            ("18446744073709551615", 0, u64::MAX),
            ("1844674407370955161.5", 1, u64::MAX),
            ("0.000000000000000000", 19, 0),
            ("007.10", 1, 71),
        ];
        for (text, scale, units) in ok {
            assert_eq!(parse_units(text, scale).unwrap(), units, "{text}");
        }
    }

    #[test]
    fn parse_units_errors() {
        let cases = [
            ("-1", 0, "Invalid number"),
            ("-0.0", 1, "Invalid number"),
            ("1.05", 1, "Value exceeds the maximum precision"),
            ("18446744073709551616", 0, "Overflow"),
            ("1844674407370955161.6", 1, "Overflow"),
            ("1", 20, "Overflow"),
            ("1", 255, "Overflow"),
            ("1e-5", 8, "Invalid number"),
            ("1E5", 0, "Invalid number"),
            ("\"1.0\"", 1, "Invalid number"),
            ("", 0, "Invalid number"),
            ("1.", 1, "Invalid number"),
            (".5", 1, "Invalid number"),
            ("1.2.3", 3, "Invalid number"),
            (" 1", 0, "Invalid number"),
        ];
        for (text, scale, message) in cases {
            assert_eq!(
                parse_units(text, scale).unwrap_err().to_string(),
                message,
                "{text}"
            );
        }
    }

    #[test]
    fn serialize_units_writes_digits() {
        let cases = [
            (0_u64, ""),
            (1, "1"),
            (807_961, "807961"),
            (807_960, "807960"),
            (46_450_888, "46450888"),
            (5_000, "5000"),
            (1_000, "1000"),
            (100_000_000, "100000000"),
            (u64::MAX, "18446744073709551615"),
        ];

        for (units, digits) in cases {
            let mut out = String::new();
            serialize_units(units, &mut out);
            assert_eq!(out, digits, "{units}");
        }

        let mut out = String::new();
        serialize_units(807_961, &mut out);
        serialize_units(5_000, &mut out);
        serialize_units(0, &mut out);
        assert_eq!(out, "8079615000");
    }
}
