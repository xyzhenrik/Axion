//! Click probe (a test aid): clicks into the page at given times, as a mouse would, so a
//! capture run can press a page's buttons (they need a real user gesture).
//!
//! `AXION_CLICK_PROBE=START_S:X,Y;START_S:X,Y;...` clicks the page at X,Y (logical pixels in the
//! page) START_S seconds after start, once per entry. `START_S:ctrl+LETTER` presses that key
//! chord on the page instead (Ctrl+C after a click that selected text).

use std::time::Duration;

/// One click.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub after: Duration,
    pub x: f32,
    pub y: f32,
    /// A Ctrl+letter chord instead of a click.
    pub ctrl: Option<char>,
}

/// Parses the value of `AXION_CLICK_PROBE`; entries that don't parse are skipped.
pub fn parse(value: &str) -> Vec<Click> {
    let mut clicks: Vec<Click> = value
        .split(';')
        .filter_map(|entry| {
            let (after, pos) = entry.trim().split_once(':')?;
            let after = Duration::from_secs_f32(after.trim().parse().ok()?);
            if let Some(letter) = pos.trim().strip_prefix("ctrl+") {
                let mut chars = letter.chars();
                let letter = chars.next().filter(|c| c.is_ascii_alphabetic() && chars.next().is_none())?;
                return Some(Click { after, x: 0.0, y: 0.0, ctrl: Some(letter) });
            }
            let (x, y) = pos.split_once(',')?;
            Some(Click { after, x: x.trim().parse().ok()?, y: y.trim().parse().ok()?, ctrl: None })
        })
        .collect();
    clicks.sort_by_key(|c| c.after);
    clicks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_are_parsed_in_time_order() {
        let clicks = parse("9:10,20; 4.5:100,200;nonsense;3:x,1");
        assert_eq!(clicks.len(), 2);
        assert_eq!(clicks[0], Click { after: Duration::from_secs_f32(4.5), x: 100.0, y: 200.0, ctrl: None });
        assert_eq!(clicks[1].after, Duration::from_secs(9));
    }

    #[test]
    fn a_key_chord_is_an_entry_too() {
        let clicks = parse("2:10,20;3:ctrl+c;4:ctrl+copy;5:ctrl+1");
        assert_eq!(clicks.len(), 2);
        assert_eq!(clicks[1], Click { after: Duration::from_secs(3), x: 0.0, y: 0.0, ctrl: Some('c') });
    }
}
