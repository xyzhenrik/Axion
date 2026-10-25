//! What the vault holds. Items are serialized to JSON and encrypted one by one.

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Login,
    Card,
    Address,
    Note,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::Login, Kind::Card, Kind::Address, Kind::Note];

    pub fn title(self) -> &'static str {
        match self {
            Kind::Login => "Passwords",
            Kind::Card => "Cards",
            Kind::Address => "Addresses",
            Kind::Note => "Secure Notes",
        }
    }
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
#[serde(default)]
pub struct Login {
    /// The site, as typed or saved (an URL or a host).
    pub site: String,
    pub username: String,
    pub password: String,
    pub notes: String,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
#[serde(default)]
pub struct Card {
    /// A name for the card ("Work Visa").
    pub label: String,
    pub cardholder: String,
    pub number: String,
    /// "MM/YY".
    pub expiry: String,
    pub cvc: String,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
#[serde(default)]
pub struct Address {
    pub name: String,
    pub street: String,
    pub city: String,
    pub zip: String,
    pub country: String,
    pub email: String,
    pub phone: String,
}

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
#[serde(default)]
pub struct Note {
    pub title: String,
    pub text: String,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, Zeroize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ItemData {
    Login(Login),
    Card(Card),
    Address(Address),
    Note(Note),
}

/// Debug never shows secrets (passwords, card numbers, notes).
impl std::fmt::Debug for ItemData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}({:?})", self.kind(), self.title())
    }
}

impl ItemData {
    pub fn kind(&self) -> Kind {
        match self {
            ItemData::Login(_) => Kind::Login,
            ItemData::Card(_) => Kind::Card,
            ItemData::Address(_) => Kind::Address,
            ItemData::Note(_) => Kind::Note,
        }
    }

    pub fn empty(kind: Kind) -> Self {
        match kind {
            Kind::Login => ItemData::Login(Login::default()),
            Kind::Card => ItemData::Card(Card::default()),
            Kind::Address => ItemData::Address(Address::default()),
            Kind::Note => ItemData::Note(Note::default()),
        }
    }

    /// What the list shows as the item's name.
    pub fn title(&self) -> String {
        let or = |a: &str, b: &str| if a.trim().is_empty() { b.to_owned() } else { a.trim().to_owned() };
        match self {
            ItemData::Login(l) => or(&crate::site::display_host(&l.site), "Login"),
            ItemData::Card(c) => or(&c.label, &or(&c.cardholder, "Card")),
            ItemData::Address(a) => or(&a.name, &or(&a.street, "Address")),
            ItemData::Note(n) => or(&n.title, "Note"),
        }
    }

    /// The list's second line — never a secret.
    pub fn subtitle(&self) -> String {
        match self {
            ItemData::Login(l) => l.username.clone(),
            ItemData::Card(c) => {
                let digits: String = c.number.chars().filter(char::is_ascii_digit).collect();
                let last = &digits[digits.len().saturating_sub(4)..];
                if last.is_empty() { String::new() } else { format!("•••• {last}") }
            }
            ItemData::Address(a) => [a.street.as_str(), a.city.as_str()]
                .iter()
                .filter(|s| !s.trim().is_empty())
                .copied()
                .collect::<Vec<_>>()
                .join(", "),
            ItemData::Note(_) => String::new(),
        }
    }

    /// Whether the search query matches (case-insensitive; never searches secrets).
    pub fn matches(&self, query: &str) -> bool {
        let query = query.trim().to_lowercase();
        if query.is_empty() {
            return true;
        }
        let haystack = match self {
            ItemData::Login(l) => format!("{} {}", l.site, l.username),
            ItemData::Card(c) => format!("{} {}", c.label, c.cardholder),
            ItemData::Address(a) => format!("{} {} {} {} {}", a.name, a.street, a.city, a.email, a.country),
            ItemData::Note(n) => n.title.clone(),
        };
        haystack.to_lowercase().contains(&query)
    }
}

/// A stored item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub id: String,
    pub data: ItemData,
    /// Unix seconds.
    pub created: i64,
    pub modified: i64,
}

impl Drop for Item {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_subtitles_and_no_secrets_in_debug() {
        let login = ItemData::Login(Login {
            site: "https://www.example.com/login".into(),
            username: "me@example.com".into(),
            password: "test-only-pass".into(),
            notes: String::new(),
        });
        assert_eq!(login.title(), "example.com");
        assert_eq!(login.subtitle(), "me@example.com");
        assert!(!format!("{login:?}").contains("test-only-pass"));
        let card = ItemData::Card(Card { number: "4111 1111 1111 1111".into(), ..Default::default() });
        assert_eq!(card.subtitle(), "•••• 1111");
        assert!(!format!("{card:?}").contains("4111"));
        assert!(login.matches("EXAMPLE"));
        assert!(!login.matches("test-only"), "passwords are not searchable");
    }
}
