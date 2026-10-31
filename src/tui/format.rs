use chrono::Duration as ChronoDuration;
use rust_decimal::Decimal;

pub(super) fn age_ms(age: ChronoDuration) -> i64 {
    age.num_milliseconds().max(0)
}

pub(super) fn format_age(ms: i64) -> String {
    let ms = ms.max(0);
    if ms < 1_000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        let tenths = ms / 100;
        format!("{}.{}s", tenths / 10, tenths % 10)
    } else {
        format!("{}m", ms / 60_000)
    }
}

pub(super) fn base_asset(symbol: &str) -> &str {
    symbol
        .split_once('/')
        .map(|(base, _)| base)
        .unwrap_or(symbol)
}

pub(super) fn quote_asset(symbol: &str) -> &str {
    symbol.split_once('/').map(|(_, quote)| quote).unwrap_or("")
}

pub(super) fn format_bps(value: Decimal) -> String {
    format!("{:.1}", value.round_dp(1))
}

/// Spread in bps. A tight spread gets three decimals so it doesn't round to zero:
/// one tick on BTC/USD is about 0.013 bps.
pub(super) fn format_spread_bps(value: Decimal) -> String {
    let dp = if value.abs() < Decimal::ONE { 3 } else { 1 };
    format!("{:.*}", dp as usize, value.round_dp(dp))
}

/// Fixed decimals with thousands separators.
pub(super) fn format_fixed(value: Decimal, dp: u32) -> String {
    group_formatted(&format!("{:.*}", dp as usize, value.round_dp(dp)))
}

pub(super) fn format_signed(value: Decimal, dp: u32) -> String {
    format!("{:+.*}", dp as usize, value.round_dp(dp))
}

/// Quantity with a precision that keeps roughly five significant digits.
pub(super) fn format_qty_auto(value: Decimal) -> String {
    let abs = value.abs();
    let dp = if abs >= Decimal::from(10_000) {
        0
    } else if abs >= Decimal::from(1_000) {
        1
    } else if abs >= Decimal::from(100) {
        2
    } else {
        3
    };
    format_fixed(value, dp)
}

pub(super) fn group_formatted(text: &str) -> String {
    let (sign, number) = if let Some(rest) = text.strip_prefix('-') {
        ("-", rest)
    } else {
        ("", text)
    };
    let Some((int_part, frac)) = number.split_once('.') else {
        return format!("{sign}{}", group_digits(number));
    };
    format!("{sign}{}.{}", group_digits(int_part), frac)
}

fn group_digits(digits: &str) -> String {
    let mut grouped = String::new();
    for (index, ch) in digits.chars().rev().enumerate() {
        if index > 0 && index % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    grouped.chars().rev().collect()
}

pub(super) fn pad_left(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text_width(text));
    format!("{}{text}", " ".repeat(pad))
}

pub(super) fn pad_right(text: &str, width: usize) -> String {
    let pad = width.saturating_sub(text_width(text));
    format!("{text}{}", " ".repeat(pad))
}

pub(super) fn text_width(text: &str) -> usize {
    text.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn groups_thousands_and_keeps_sign() {
        assert_eq!(format_fixed(Decimal::new(-7723255, 2), 2), "-77,232.55");
        assert_eq!(format_fixed(Decimal::new(5, 0), 1), "5.0");
    }

    #[test]
    fn signed_always_shows_sign() {
        assert_eq!(format_signed(Decimal::new(5, 1), 2), "+0.50");
        assert_eq!(format_signed(Decimal::new(-5, 1), 1), "-0.5");
    }

    #[test]
    fn tight_spreads_keep_precision() {
        assert_eq!(format_spread_bps(Decimal::new(129, 4)), "0.013");
        assert_eq!(format_spread_bps(Decimal::new(125, 2)), "1.2");
    }

    #[test]
    fn ages_format_by_magnitude() {
        assert_eq!(format_age(12), "12ms");
        assert_eq!(format_age(1_250), "1.2s");
        assert_eq!(format_age(120_000), "2m");
    }
}
