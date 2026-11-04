//! Every block of UI action ids in one table, so two widgets can never share an id unnoticed.
//!
//! A click carries only an [`ActionId`]; the browser hands it to the first owner that claims it.
//! Two owners with overlapping ranges therefore steal each other's clicks: the password eyes'
//! block began at 13 000, where the sidebar's Downloads rows have their buttons too, so the first
//! rows' "Show in Folder" button never fired. New widgets take a block from the free space, add it
//! to [`RANGES`] and use that base; the tests fail on overlaps and on any literal base in the
//! sources that no block covers.

#[cfg(test)]
use axion_ui::ActionId;

/// The History sheet: [`crate::list_sheet::SPAN`] ids.
pub const HISTORY_SHEET: u32 = 90_000;
/// The Downloads sheet.
pub const DOWNLOADS_SHEET: u32 = 94_000;
/// The password eyes.
pub const EYES: u32 = 52_000;

#[cfg(test)]
/// One owner's block of ids: `[start, end)`.
pub struct Block {
    pub owner: &'static str,
    pub start: u32,
    pub end: u32,
}

#[cfg(test)]
const fn block(owner: &'static str, start: u32, end: u32) -> Block {
    Block { owner, start, end }
}

/// All blocks. Settings' pages carry ids relative to its base, sparse inside 8000-9000; the pages
/// that stand beside it are listed apart and may lie among those numbers (see `nested`).
#[cfg(test)]
pub const RANGES: &[Block] = &[
    block("chrome", 1, 40),
    block("chrome tab select", 1000, 2000),
    block("chrome tab close", 2000, 3000),
    block("overview", 3000, 5000),
    block("suggestions", 5000, 6000),
    block("page menu", 6000, 6050),
    block("account menu", 6050, 6100),
    block("shields popover", 6100, 6200),
    block("downloads popover", 6200, 6300),
    block("extensions popover", 6300, 6400),
    block("find", 6400, 6500),
    block("palette", 6500, 6600),
    block("playlist popover", 6600, 6700),
    block("permission prompt", 6700, 6800),
    block("reader popover", 6800, 6900),
    block("translate popover", 6900, 6950),
    block("network popover", 6950, 6970),
    block("circuit popover", 6970, 7000),
    block("chrome panel rows", 7000, 8000),
    block("settings", 8000, 9000),
    block("lock screen", 9000, 9010),
    block("settings import", 9020, 9040),
    block("settings reading", 9100, 9140),
    block("overview header", 9200, 9210),
    block("update alert", 9250, 9260),
    block("toast", 9210, 9220),
    block("print dialog", 9300, 9400),
    block("screenshot", 9400, 9500),
    block("player", 9500, 9600),
    block("accounts", 9650, 9700),
    block("pip", 9700, 9800),
    block("settings network", 9800, 9900),
    block("chrome panel delete", 10_000, 11_000),
    block("chrome tab groups", 11_000, 12_000),
    block("chrome other tabs", 12_000, 13_000),
    block("chrome panel buttons", 13_000, 17_000),
    block("context menus", 17_000, 18_000),
    block("chrome bookmarks", 18_000, 19_000),
    block("passwords", 20_000, 21_100),
    block("onboarding", 30_000, 31_000),
    block("playlist page", 40_000, 49_000),
    block("eyes", EYES, EYES + 256),
    block("chrome tab mute", 60_000, 62_000),
    block("group sheet", 70_000, 71_000),
    block("tab search", 80_000, 81_000),
    block("history sheet", HISTORY_SHEET, HISTORY_SHEET + crate::list_sheet::SPAN),
    block("downloads sheet", DOWNLOADS_SHEET, DOWNLOADS_SHEET + crate::list_sheet::SPAN),
    block("chrome tab group chips", 100_000, 200_000),
];

#[cfg(test)]
/// The owners whose block contains `action`; exactly one for a well-formed id.
pub fn owners(action: ActionId) -> Vec<&'static str> {
    RANGES.iter().filter(|b| (b.start..b.end).contains(&action.0)).map(|b| b.owner).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Blocks that overlap Settings' sparse numbering on purpose: Settings has no ids in those
    /// places (its pages' ids end where these begin, see `settings.rs`), so they never meet.
    fn nested(a: &Block, b: &Block) -> bool {
        let pair = |x: &str, y: &str| (a.owner == x && b.owner == y) || (a.owner == y && b.owner == x);
        ["settings import", "settings reading", "settings network"]
            .iter()
            .any(|inner| pair("settings", inner))
    }

    #[test]
    fn blocks_do_not_overlap() {
        for (i, a) in RANGES.iter().enumerate() {
            assert!(a.start < a.end, "{}: empty block", a.owner);
            for b in &RANGES[i + 1..] {
                let overlap = a.start < b.end && b.start < a.end;
                assert!(
                    !overlap || nested(a, b),
                    "{} [{}, {}) overlaps {} [{}, {})",
                    a.owner,
                    a.start,
                    a.end,
                    b.owner,
                    b.start,
                    b.end
                );
            }
        }
    }

    /// The panel buttons are the ids the password eyes once swallowed: every one of them has the
    /// chrome as its only owner, and no eye claims it.
    #[test]
    fn panel_buttons_have_one_owner() {
        for i in 0..64 {
            for k in 0..crate::chrome::PANEL_BUTTONS {
                let id = crate::chrome::ChromeAction::PanelButton(i, k).id();
                assert_eq!(owners(id), ["chrome panel buttons"], "PanelButton({i}, {k}) = {}", id.0);
                assert!(!crate::eye::owns(id), "an eye claims PanelButton({i}, {k})");
            }
        }
    }

    fn sources(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                sources(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Every literal action base in the sources (`const X_BASE: u32 = 6400;`, `ActionId(9000)`)
    /// lies inside a block of the table, so a new widget cannot pick a number on its own.
    #[test]
    fn literal_bases_are_registered() {
        let mut files = Vec::new();
        sources(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut files);
        let mut checked = 0;
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap_or_default();
            for (n, line) in text.lines().enumerate() {
                let line = line.trim();
                let is_const = line.starts_with("const ")
                    || line.starts_with("pub const ")
                    || line.starts_with("pub(crate) const ");
                let is_base = line.contains("BASE: u32 =") || line.contains(": ActionId = ActionId(");
                if !is_const || !is_base {
                    continue;
                }
                let Some(rhs) = line.split('=').nth(1) else { continue };
                // Only literals: a base written as `OTHER + 1` is relative to a block already.
                let rhs = rhs.trim_start().trim_start_matches("ActionId(");
                if !rhs.starts_with(|c: char| c.is_ascii_digit()) {
                    continue;
                }
                let digits: String = rhs
                    .chars()
                    .take_while(|c| c.is_ascii_digit() || *c == '_')
                    .filter(|c| *c != '_')
                    .collect();
                let Ok(value) = digits.parse::<u32>() else { continue };
                checked += 1;
                // Bases relative to a parent block (Settings' pages, 1020 + 8000) are small.
                let relative =
                    file.ends_with("settings.rs") || file.parent().is_some_and(|p| p.ends_with("settings"));
                assert!(
                    relative || RANGES.iter().any(|b| (b.start..b.end).contains(&value)),
                    "{}:{}: {value} is in no block of action_ids::RANGES: {line}",
                    file.display(),
                    n + 1
                );
            }
        }
        assert!(checked > 20, "the scan found {checked} bases");
    }
}
