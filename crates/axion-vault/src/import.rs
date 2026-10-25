//! Importing logins from other browsers' and managers' CSV exports: Chrome / Edge / Brave
//! (name,url,username,password,note), Firefox (url,username,password,…) and Bitwarden
//! (…,type,name,notes,…,login_uri,login_username,login_password,…). Columns are found by
//! their header names, so column order doesn't matter.

use crate::item::{ItemData, Login, Note};

/// Splits CSV text into records (RFC 4180: quoted fields, "" escapes, newlines in quotes).
pub fn parse_csv(text: &str) -> Vec<Vec<String>> {
    let mut records = Vec::new();
    let mut record = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            (true, '"') => quoted = false,
            (true, c) => field.push(c),
            (false, '"') if field.is_empty() => quoted = true,
            (false, ',') => record.push(std::mem::take(&mut field)),
            (false, '\r') => {}
            (false, '\n') => {
                record.push(std::mem::take(&mut field));
                if record.iter().any(|f| !f.is_empty()) {
                    records.push(std::mem::take(&mut record));
                }
                record.clear();
            }
            (false, c) => field.push(c),
        }
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records
}

/// The items in an exported CSV file (unknown formats give nothing).
pub fn items_from_csv(text: &str) -> Vec<ItemData> {
    let mut records = parse_csv(text.trim_start_matches('\u{feff}')).into_iter();
    let Some(header) = records.next() else { return Vec::new() };
    let column =
        |names: &[&str]| header.iter().position(|h| names.contains(&h.trim().to_lowercase().as_str()));
    let url = column(&["url", "login_uri", "website", "uri"]);
    let user = column(&["username", "login_username", "user", "email"]);
    let pass = column(&["password", "login_password"]);
    let note = column(&["note", "notes", "comment"]);
    let name = column(&["name", "title"]);
    let kind = column(&["type"]);
    let get = |r: &[String], i: Option<usize>| i.and_then(|i| r.get(i)).cloned().unwrap_or_default();
    records
        .filter_map(|r| {
            // Bitwarden exports notes as type "note"; everything else with a password is a login.
            if get(&r, kind).eq_ignore_ascii_case("note") {
                return Some(ItemData::Note(Note { title: get(&r, name), text: get(&r, note) }));
            }
            let password = get(&r, pass);
            if password.is_empty() {
                return None;
            }
            let site = get(&r, url);
            let site = if site.is_empty() { get(&r, name) } else { site };
            Some(ItemData::Login(Login { site, username: get(&r, user), password, notes: get(&r, note) }))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chrome_firefox_bitwarden() {
        let chrome = "name,url,username,password,note\nexample.com,https://example.com/,me,\"p,w\"\"1\",\n";
        let items = items_from_csv(chrome);
        let ItemData::Login(l) = &items[0] else { panic!() };
        assert_eq!(
            (l.site.as_str(), l.username.as_str(), l.password.as_str()),
            ("https://example.com/", "me", "p,w\"1")
        );

        let firefox = "\"url\",\"username\",\"password\",\"httpRealm\",\"formActionOrigin\",\"guid\"\n\"https://a.org\",\"u\",\"pw\",,\"https://a.org\",\"{1}\"\n";
        assert_eq!(items_from_csv(firefox).len(), 1);

        let bitwarden = "folder,favorite,type,name,notes,fields,reprompt,login_uri,login_username,login_password,login_totp\n\
                         ,,login,Site,,,0,https://s.net,bob,secret,\n,,note,My note,\"line1\nline2\",,0,,,,\n";
        let items = items_from_csv(bitwarden);
        assert_eq!(items.len(), 2);
        let ItemData::Note(n) = &items[1] else { panic!() };
        assert_eq!(n.text, "line1\nline2");
    }
}
