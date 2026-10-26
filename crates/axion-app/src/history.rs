//! The History panel: entries grouped by the day they were last visited (newest first), with
//! headings like Safari's ("Today", "Yesterday", "Saturday, 24 October").

use axion_i18n::tr;
use axion_store::HistoryEntry;
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

use crate::chrome::PanelItem;
use crate::url::display_host;

/// The panel rows for `entries` (newest first), each day starting with its heading.
pub fn panel_items(entries: Vec<HistoryEntry>, now: i64) -> Vec<PanelItem> {
    let tz = TimeZone::system();
    let today = local_date(now, &tz);
    let mut current: Option<Date> = None;
    entries
        .into_iter()
        .map(|entry| {
            let day = local_date(entry.last_visit, &tz);
            let section = (current != day)
                .then(|| day.map_or_else(|| tr("Earlier").to_owned(), |d| day_label(d, today)));
            current = day;
            PanelItem {
                detail: display_host(&entry.url),
                title: entry.title,
                url: entry.url,
                section,
                deletable: true,
                ..PanelItem::default()
            }
        })
        .collect()
}

pub(crate) fn local_date(unix: i64, tz: &TimeZone) -> Option<Date> {
    Some(Timestamp::from_second(unix).ok()?.to_zoned(tz.clone()).date())
}

/// "Today", "Yesterday", "Saturday, 24 October", or with the year when it isn't this year.
pub(crate) fn day_label(day: Date, today: Option<Date>) -> String {
    if let Some(today) = today {
        if day == today {
            return tr("Today").into();
        }
        if today.yesterday().ok() == Some(day) {
            return tr("Yesterday").into();
        }
        if day.year() == today.year() {
            return day.strftime("%A, %-d %B").to_string();
        }
    }
    day.strftime("%A, %-d %B %Y").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    #[test]
    fn labels_days() {
        let today = date(2026, 10, 25);
        assert_eq!(day_label(today, Some(today)), "Today");
        assert_eq!(day_label(date(2026, 10, 24), Some(today)), "Yesterday");
        assert_eq!(day_label(date(2026, 10, 23), Some(today)), "Friday, 23 October");
        assert_eq!(day_label(date(2025, 12, 31), Some(today)), "Wednesday, 31 December 2025");
    }

    #[test]
    fn groups_entries_by_day() {
        let entry = |url: &str, at: i64| HistoryEntry {
            url: url.into(),
            title: String::new(),
            visit_count: 1,
            last_visit: at,
        };
        let tz = TimeZone::system();
        // Noon of a fixed day in local time, so the test doesn't depend on the clock.
        let noon = date(2026, 10, 25).at(12, 0, 0, 0).to_zoned(tz).map_or(0, |z| z.timestamp().as_second());
        let items = panel_items(
            vec![
                entry("https://a/", noon),
                entry("https://b/", noon - 60),
                entry("https://c/", noon - 86_400),
            ],
            noon,
        );
        let sections: Vec<Option<&str>> = items.iter().map(|i| i.section.as_deref()).collect();
        assert_eq!(sections, [Some("Today"), None, Some("Yesterday")]);
        assert!(items.iter().all(|i| i.deletable));
    }
}
