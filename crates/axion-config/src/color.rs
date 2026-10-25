use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// An sRGB colour, written in config files as `"#RRGGBB"` or `"#RRGGBBAA"`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Color {
    pub const fn rgb(hex: u32) -> Self {
        Self { r: (hex >> 16) as u8, g: (hex >> 8) as u8, b: hex as u8, a: 255 }
    }

    pub const fn with_alpha(self, a: f32) -> Self {
        Self { a: (a * 255.0 + 0.5) as u8, ..self }
    }

    /// Components in 0..=1, still sRGB-encoded (the compositor blends in that space).
    pub fn to_f32(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a].map(|c| c as f32 / 255.0)
    }
}

impl FromStr for Color {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let hex = s.strip_prefix('#').ok_or_else(|| format!("colour {s:?} must start with '#'"))?;
        let value = u32::from_str_radix(hex, 16).map_err(|_| format!("colour {s:?} is not hexadecimal"))?;
        match hex.len() {
            6 => Ok(Self::rgb(value)),
            8 => Ok(Self { a: value as u8, ..Self::rgb(value >> 8) }),
            _ => Err(format!("colour {s:?} must be #RRGGBB or #RRGGBBAA")),
        }
    }
}

impl fmt::Display for Color {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "#{:02X}{:02X}{:02X}", self.r, self.g, self.b)?;
        if self.a != 255 {
            write!(f, "{:02X}", self.a)?;
        }
        Ok(())
    }
}

impl Serialize for Color {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_prints() {
        let c: Color = "#2E6BFF".parse().unwrap();
        assert_eq!(c, Color::rgb(0x2E6BFF));
        assert_eq!(c.to_string(), "#2E6BFF");
        let t: Color = "#16181E80".parse().unwrap();
        assert_eq!(t.a, 0x80);
        assert_eq!(t.to_string(), "#16181E80");
        assert!("2E6BFF".parse::<Color>().is_err());
        assert!("#12345".parse::<Color>().is_err());
    }
}
