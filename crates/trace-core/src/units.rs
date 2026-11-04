use serde::{Deserialize, Serialize};
use std::fmt;

/// A length stored as whole nanometres, so geometry is exact and comparable.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub struct Length(pub i64);

impl Length {
    pub const NM_PER_MIL: f64 = 25_400.0;
    pub const NM_PER_MM: f64 = 1_000_000.0;

    pub fn from_mil(mil: f64) -> Self {
        Length((mil * Self::NM_PER_MIL).round() as i64)
    }
    pub fn from_mm(mm: f64) -> Self {
        Length((mm * Self::NM_PER_MM).round() as i64)
    }
    pub fn mil(self) -> f64 {
        self.0 as f64 / Self::NM_PER_MIL
    }
    pub fn mm(self) -> f64 {
        self.0 as f64 / Self::NM_PER_MM
    }

    /// Parses strings such as `12.6mil`, `0.35mm`, `1.4um` or a bare number in mil.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let (num, unit) = match s.find(|c: char| c.is_ascii_alphabetic()) {
            Some(i) => (&s[..i], s[i..].to_ascii_lowercase()),
            None => (s, "mil".to_string()),
        };
        let v: f64 = num.trim().parse().ok()?;
        match unit.as_str() {
            "mil" => Some(Self::from_mil(v)),
            "mm" => Some(Self::from_mm(v)),
            "um" => Some(Length((v * 1000.0).round() as i64)),
            "in" | "inch" => Some(Self::from_mil(v * 1000.0)),
            _ => None,
        }
    }
}

impl fmt::Display for Length {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.3} mm", self.mm())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_units() {
        assert_eq!(Length::parse("1mil"), Some(Length(25_400)));
        assert_eq!(Length::parse("0.1mm"), Some(Length(100_000)));
        assert_eq!(Length::parse("12.6mil").unwrap().mil(), 12.6);
        assert_eq!(Length::parse(" 10 "), Some(Length(254_000)));
        assert_eq!(Length::parse("abc"), None);
    }
}
