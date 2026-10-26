//! Single-line text editing (cursor, selection, insert/delete), independent of rendering.

use std::ops::Range;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Editor {
    text: String,
    /// Byte index of the caret; always on a char boundary.
    cursor: usize,
    /// Other end of the selection, if any.
    anchor: Option<usize>,
}

impl Editor {
    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Replaces the content and puts the caret at the end.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.replace(['\n', '\r'], " ");
        self.cursor = self.text.len();
        self.anchor = None;
    }

    /// Selects the word around byte index `at` (letters and digits; URL punctuation such as
    /// `/`, `.`, `:` and `?` ends a word, so a double click picks one part of an address).
    pub fn select_word_at(&mut self, at: usize) {
        let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
        let at = at.min(self.text.len());
        let start = self.text[..at]
            .char_indices()
            .rev()
            .find(|&(_, c)| !is_word(c))
            .map_or(0, |(i, c)| i + c.len_utf8());
        let end = self.text[at..]
            .char_indices()
            .find(|&(_, c)| !is_word(c))
            .map_or(self.text.len(), |(i, _)| at + i);
        self.anchor = Some(start);
        self.cursor = end.max(start);
    }

    pub fn select_all(&mut self) {
        self.anchor = Some(0);
        self.cursor = self.text.len();
    }

    pub fn selection(&self) -> Option<Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.cursor).then(|| anchor.min(self.cursor)..anchor.max(self.cursor))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|r| &self.text[r])
    }

    fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some(range) => {
                self.cursor = range.start;
                self.text.replace_range(range, "");
                self.anchor = None;
                true
            }
            None => {
                self.anchor = None;
                false
            }
        }
    }

    /// Types `s` over the selection. Line breaks become spaces (single-line field).
    pub fn insert(&mut self, s: &str) {
        self.delete_selection();
        let clean: String = s
            .chars()
            .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
            .filter(|c| !c.is_control())
            .collect();
        self.text.insert_str(self.cursor, &clean);
        self.cursor += clean.len();
    }

    pub fn backspace(&mut self) {
        if !self.delete_selection()
            && let Some(prev) = self.prev_boundary(self.cursor)
        {
            self.text.replace_range(prev..self.cursor, "");
            self.cursor = prev;
        }
    }

    pub fn delete(&mut self) {
        if !self.delete_selection()
            && let Some(next) = self.next_boundary(self.cursor)
        {
            self.text.replace_range(self.cursor..next, "");
        }
    }

    /// Deletes the word before the caret (Ctrl+Backspace).
    pub fn backspace_word(&mut self) {
        if !self.delete_selection() {
            let start = self.word_start(self.cursor);
            self.text.replace_range(start..self.cursor, "");
            self.cursor = start;
        }
    }

    pub fn move_left(&mut self, select: bool, by_word: bool) {
        let target =
            if by_word { Some(self.word_start(self.cursor)) } else { self.prev_boundary(self.cursor) };
        match (self.selection(), select) {
            // Collapsing a selection goes to its start.
            (Some(range), false) => self.move_to(range.start, false),
            _ => self.move_to(target.unwrap_or(0), select),
        }
    }

    pub fn move_right(&mut self, select: bool, by_word: bool) {
        let target = if by_word { Some(self.word_end(self.cursor)) } else { self.next_boundary(self.cursor) };
        match (self.selection(), select) {
            (Some(range), false) => self.move_to(range.end, false),
            _ => self.move_to(target.unwrap_or(self.text.len()), select),
        }
    }

    pub fn home(&mut self, select: bool) {
        self.move_to(0, select);
    }

    pub fn end(&mut self, select: bool) {
        self.move_to(self.text.len(), select);
    }

    /// Moves the caret to byte index `to` (clamped to a char boundary).
    pub fn move_to(&mut self, mut to: usize, select: bool) {
        to = to.min(self.text.len());
        while !self.text.is_char_boundary(to) {
            to -= 1;
        }
        if select {
            self.anchor.get_or_insert(self.cursor);
        } else {
            self.anchor = None;
        }
        self.cursor = to;
    }

    fn prev_boundary(&self, i: usize) -> Option<usize> {
        self.text[..i].char_indices().next_back().map(|(j, _)| j)
    }

    fn next_boundary(&self, i: usize) -> Option<usize> {
        self.text[i..].chars().next().map(|c| i + c.len_utf8())
    }

    fn word_start(&self, i: usize) -> usize {
        let before = &self.text[..i];
        let trimmed = before.trim_end_matches(|c: char| !c.is_alphanumeric());
        trimmed
            .rfind(|c: char| !c.is_alphanumeric())
            .map_or(0, |j| j + trimmed[j..].chars().next().map_or(1, char::len_utf8))
    }

    fn word_end(&self, i: usize) -> usize {
        let after = &self.text[i..];
        let skip = after.len() - after.trim_start_matches(|c: char| !c.is_alphanumeric()).len();
        let rest = &after[skip..];
        i + skip + rest.find(|c: char| !c.is_alphanumeric()).unwrap_or(rest.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn editor(text: &str) -> Editor {
        let mut e = Editor::default();
        e.set_text(text);
        e
    }

    #[test]
    fn typing_replaces_selection() {
        let mut e = editor("https://example.com");
        e.select_all();
        e.insert("rust lang");
        assert_eq!(e.text(), "rust lang");
        assert_eq!(e.cursor(), 9);
    }

    #[test]
    fn backspace_and_delete_handle_multibyte() {
        let mut e = editor("añb");
        e.move_left(false, false);
        e.backspace();
        assert_eq!(e.text(), "ab");
        e.home(false);
        e.delete();
        assert_eq!(e.text(), "b");
    }

    #[test]
    fn word_motion() {
        let mut e = editor("hello wide world");
        e.backspace_word();
        assert_eq!(e.text(), "hello wide ");
        e.move_left(false, true);
        assert_eq!(e.cursor(), 6);
        e.move_right(true, true);
        assert_eq!(e.selected_text(), Some("wide"));
    }

    #[test]
    fn arrows_collapse_selection() {
        let mut e = editor("abcdef");
        e.select_all();
        e.move_left(false, false);
        assert_eq!((e.cursor(), e.selection()), (0, None));
        e.end(true);
        e.move_right(false, false);
        assert_eq!(e.cursor(), 6);
    }

    #[test]
    fn newlines_become_spaces() {
        let mut e = editor("");
        e.insert("a\nb\tc");
        assert_eq!(e.text(), "a bc");
    }

    #[test]
    fn double_click_selects_one_part_of_an_address() {
        let mut e = editor("https://www.rust-lang.org/learn");
        e.select_word_at(14);
        assert_eq!(e.selected_text(), Some("rust-lang"));
        e.select_word_at(28);
        assert_eq!(e.selected_text(), Some("learn"));
    }
}
