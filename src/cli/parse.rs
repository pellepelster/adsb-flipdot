//! Value parsers for numeric command line options.

use std::ops::RangeInclusive;

fn number(s: &str) -> Result<f64, String> {
    s.parse().map_err(|_| format!("`{s}` is not a number"))
}

/// A number that lies in `range`.
pub fn number_in(s: &str, range: RangeInclusive<f64>) -> Result<f64, String> {
    let value = number(s)?;
    if range.contains(&value) {
        Ok(value)
    } else {
        Err(format!(
            "must be between {} and {}",
            range.start(),
            range.end()
        ))
    }
}

/// A number greater than zero.
pub fn positive(s: &str) -> Result<f64, String> {
    let value = number(s)?;
    if value.is_finite() && value > 0.0 {
        Ok(value)
    } else {
        Err("must be greater than 0".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_in_accepts_the_ends_of_the_range() {
        assert_eq!(number_in("-90", -90.0..=90.0), Ok(-90.0));
        assert_eq!(number_in("90", -90.0..=90.0), Ok(90.0));
        assert_eq!(number_in("12.5", -90.0..=90.0), Ok(12.5));
        assert_eq!(
            number_in("91", -90.0..=90.0),
            Err("must be between -90 and 90".to_string())
        );
        assert!(number_in("NaN", -90.0..=90.0).is_err());
        assert!(number_in("inf", 0.0..=360.0).is_err());
        assert_eq!(
            number_in("north", 0.0..=360.0),
            Err("`north` is not a number".to_string())
        );
    }

    #[test]
    fn positive_refuses_zero_and_below() {
        assert_eq!(positive("0.5"), Ok(0.5));
        assert!(positive("0").is_err());
        assert!(positive("-2").is_err());
        assert!(positive("inf").is_err());
        assert!(positive("fast").is_err());
    }
}
