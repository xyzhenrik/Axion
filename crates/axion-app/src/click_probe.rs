//! Click probe (a test aid): clicks into the page at given times, as a mouse would, so a
//! capture run can press a page's buttons (they need a real user gesture).
//!
//! `AXION_CLICK_PROBE=START_S:X,Y;START_S:X,Y;...` clicks the page at X,Y (logical pixels in the
//! page) START_S seconds after start, once per entry.

use std::time::Duration;

/// One click.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Click {
    pub after: Duration,
    pub x: f32,
    pub y: f32,
}

/// Parses the value of `AXION_CLICK_PROBE`; entries that don't parse are skipped.
pub fn parse(value: &str) -> Vec<Click> {
    let mut clicks: Vec<Click> = value
        .split(';')
        .filter_map(|entry| {
            let (after, pos) = entry.trim().split_once(':')?;
            let (x, y) = pos.split_once(',')?;
            Some(Click {
                after: Duration::from_secs_f32(after.trim().parse().ok()?),
                x: x.trim().parse().ok()?,
                y: y.trim().parse().ok()?,
            })
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
        assert_eq!(clicks[0], Click { after: Duration::from_secs_f32(4.5), x: 100.0, y: 200.0 });
        assert_eq!(clicks[1].after, Duration::from_secs(9));
    }
}
