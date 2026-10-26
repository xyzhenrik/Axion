# Translation guide (German, `de.toml`)

Instructions for whoever fills `de.toml` (stage 2).

## File format

- One entry per line: `"English source text" = "Deutsche Übersetzung"`. The key (left side) is the
  exact English text shown in the UI and **must not be edited**. Fill only the right side.
- An empty value (`""`) means "not translated yet"; the app then shows the English text.
- Lines starting with `#` are comments (they name the source file the key comes from). Keep them.
- Only change values. Do not add, remove, sort or rename keys (the test would fail; new keys come
  from `cargo run -p axion-i18n --example extract`).
- TOML basic strings: escape `"` as `\"` and `\` as `\\`. Use the real characters `ä ö ü ß “ ” … ·`.
- Check with `cargo test -p axion-i18n` (all keys present, placeholders match, no duplicates).

## Placeholders

`{name}`, `{n}`, `{count}`, ... in a key are filled by the program. The translation must contain
**exactly the same placeholders** (same spelling), in any order, e.g.

    "{index} of {count}" = "{index} von {count}"
    "Delete {count} items?" = "{count} Objekte löschen?"

Never translate the text inside `{}`. Plurals come as two keys (`"{n} Tab"` / `"{n} Tabs"`): translate
each as the singular / plural form (`"{n} Tab"` / `"{n} Tabs"`).

## Style

- Informal address is not needed: use neutral phrasing (infinitive or imperative "Sie"-free:
  "Passwort eingeben", "Tab schließen"), like Safari in German.
- Keep texts about as short as the English: buttons, segmented controls, tab bar and sidebar rows are
  fixed-width and clip (`…`) otherwise. Prefer short words (`Std.`, `Min.`) in sliders/value labels.
- Title Case in English for menu items/buttons becomes normal German capitalisation (nouns capital).
- Keep trailing `…` and `:` and `·` separators as in the key. Keep curly quotes `“{query}”`.
- Do not translate: Axion, Shields, Liquid Glass, DuckDuckGo/Google/Brave/Startpage/Ecosia,
  keyboard names (Ctrl, Shift, Cmd, Alt, Esc), file formats, `PiP`.
- Same English word, different meaning: check the `# file` comment above it for context
  (e.g. "Lock" = verb in a menu; "Private" = private browsing; "Open" = verb).

## Glossary

| English | German |
|---|---|
| Tab / Tabs | Tab / Tabs |
| Sidebar | Seitenleiste |
| Tab Overview | Tab-Übersicht |
| Shields | Shields (unchanged) |
| Playlist | Playlist |
| Bookmarks / Bookmark | Lesezeichen / Lesezeichen (Verb: "Lesezeichen setzen") |
| History | Verlauf |
| Reading List | Leseliste |
| Downloads | Downloads |
| Passwords | Passwörter |
| Private / Private Browsing | Privat / Privates Surfen |
| Start Page / Home | Startseite |
| Settings | Einstellungen |
| Account | Account |
| Search | Suchen (Verb/Feld) / Suche (Titel) |
| Find on Page | Auf Seite suchen |
| Reload | Neu laden |
| Back / Forward | Zurück / Vorwärts |
| Appearance | Erscheinungsbild |
| Privacy | Datenschutz |
| Keyboard | Tastatur |
| Media | Medien |
| Cookies | Cookies |
| Blocked | Blockiert |
| Trackers / Ads | Tracker / Werbung |
| Vault | Tresor |
| Master Password | Hauptpasswort |
| Lock / Unlock | Sperren / Entsperren |
| Pin / Unpin | Anheften / Loslösen |
| Reduce Transparency / Reduce Motion | Transparenz reduzieren / Bewegung reduzieren |
| Accent color | Akzentfarbe |
| Picture in Picture | Bild im Bild |
| Fullscreen | Vollbild |
| Freeze / Discard (tabs) | Einfrieren / Verwerfen |
| Export / Import | Exportieren / Importieren |
| Cancel / Save / Add / Remove / Done | Abbrechen / Sichern / Hinzufügen / Entfernen / Fertig |
