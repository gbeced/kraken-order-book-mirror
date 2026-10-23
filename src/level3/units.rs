use rust_decimal::Decimal;
use std::error::Error;
use std::fmt::Write;

pub fn to_units(value: Decimal, scale: u8) -> Result<u64, Box<dyn Error>> {
    if value.is_sign_negative() {
        return Err("negative value".into());
    }
    let scale = u32::from(scale);
    let mantissa = value.mantissa();
    let value_scale = value.scale();
    let units = if value_scale <= scale {
        let factor = 10i128
            .checked_pow(scale - value_scale)
            .ok_or_else(|| -> Box<dyn Error> { "scale factor overflow".into() })?;
        mantissa
            .checked_mul(factor)
            .ok_or_else(|| -> Box<dyn Error> { "unit count overflow".into() })?
    } else {
        let factor = 10i128
            .checked_pow(value_scale - scale)
            .ok_or_else(|| -> Box<dyn Error> { "scale factor overflow".into() })?;
        if mantissa % factor != 0 {
            return Err("value is not a multiple of the increment".into());
        }
        mantissa / factor
    };
    u64::try_from(units).map_err(|_| -> Box<dyn Error> { "unit count does not fit in u64".into() })
}

pub fn to_decimal(units: u64, scale: u8) -> Decimal {
    Decimal::from_i128_with_scale(units as i128, u32::from(scale))
}

pub fn serialize_units(units: u64, out: &mut String) {
    if units != 0 {
        write!(out, "{units}").unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    #[test]
    fn order_price_and_qty_round_trip() {
        let cases = [
            ("1", 0 as u8, 1 as u64),
            ("1", 8, 100000000),
            ("80796.1", 1, 807961),
            ("80796.1", 2, 8079610),
        ];

        for (str_value, scale, units) in cases {
            assert_eq!(
                to_units(Decimal::from_str(str_value).unwrap(), scale).unwrap(),
                units
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
