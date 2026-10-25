# Reference: Safari on iPadOS 26 (user's screen recordings, 2026-10-25)

Observed frame by frame in three recordings the user made on their iPad. This is what Axion
matches; deviations are deliberate only where noted (no iCloud, desktop input).

## Liquid Glass switch (toggle)

Timeline at 47 fps, tap-and-drag on "Farbe in Tableiste anzeigen":

| t (ms) | What happens |
|---|---|
| 0 | Press. Knob is the usual opaque white pill on the green track. |
| ~50–100 | The knob **morphs into a clear glass lens**: ~1.45× the knob's width, ~1.4× its height, so it sticks out above, below and past the track's end. The opaque white is gone: the track shows through, magnified, with a dark refraction line along the lens's left inner edge, a bright rim, and a soft drop shadow beneath. |
| drag | The lens follows the pointer horizontally (spring, no lag spikes), can be pulled slightly past both ends (rubber band). The track colour fades green → grey continuously with the lens position. The lens squashes slightly along the motion. |
| release | The lens springs to the nearest end, then **refrosts**: over ~150 ms it becomes milky (blurred white) and settles back into the opaque white knob at normal size. |

A plain click (no drag) plays the same: swell into the lens, travel to the other end, refrost.
Sliders behave the same way (the round knob becomes a lens while dragged).

## Player (Safari's own video player, fullscreen)

- Black backdrop; video letterboxed.
- **Top left:** two dark glass circles (≈36 px): close (×) and Picture-in-Picture.
- **Top right:** dark glass capsule (≈180×36) with a volume slider (white fill, grey track) and
  a speaker icon at its right end.
- **Centre:** three dark glass circles side by side: skip back 10 s (≈56 px, arrow with "10"),
  play/pause (≈80 px, larger glyph), skip forward 10 s. Pressing one swells it (press glass).
- **Bottom:** one full-width dark glass capsule (≈40 px tall) with elapsed time at the left,
  a thin white/grey progress line, and remaining time ("−49:40") at the right — inside the capsule.
- **Bottom right, above the bar:** a small glass circle (speedometer) opening a dark glass menu
  "Wiedergabetempo": 2×, 1,5×, 1,25×, ✓ 1×, 0,5×.
- Subtitles are drawn by the page above the bar.
- Controls fade out after a few seconds of no input and back in on movement.
- Leaving fullscreen into PiP: the video shrinks into a floating rounded window (top right) with
  ×, PiP-return, ±10 s, play/pause and a thin progress line; the page plays on behind.
- While a tab plays audio: a blue speaker icon sits in the address field (left of the host) and
  a speaker icon on the tab's segment; clicking it mutes the tab.
- (YouTube's *desktop* site keeps its own player, Safari only shows the banner
  "„youtube.com" ist im Vollbildmodus". Axion keeps trusted sites' players — user decision.)
- Axion uses this player only in fullscreen; inline, pages keep their own player (user
  decision, 2026-10-25).

## Private browsing (locked)

- Tab overview with "Privat" selected in the top segmented control: soft grey-pink backdrop,
  centred grey **lock icon**, text **„Privates Surfen" ist gesperrt**, blue capsule button
  **Entsperren** below.
- Pressing Entsperren shows a centred glass alert card (rounded ~20 px): icon, bold title
  ("Touch ID zum Entsperren von „Privates Surfen" verwenden"), grey explanation, and a grey
  capsule button "Abbrechen". It appears with a quick scale-up + fade and disappears the same way.
  Axion uses its password field in this card instead of Touch ID.
- Sidebar: "Privat" row with a lock icon at the right; selected row is a grey capsule.

## Tab overview

- Opening: the whole current page shrinks as one card (with shadow) into its grid slot
  (~300 ms spring), the backdrop fades to a soft tinted grey, other thumbnails fade in.
- Closing: the chosen card grows back to the full window, overshooting a hair.
- **Top bar:** left: sidebar button and a "…" circle; centre: a glass capsule segmented control
  **[Privat | N Tabs]** (selected segment is a lighter pill); right: **+**, search (magnifier) and
  a **blue filled circle with a checkmark** (done).
- Cards: rounded ~10 px, × in a small circle at the top right, favicon + title centred below.

## Toolbars

- **Separate tab bar** (Safari's default): row 1 = sidebar, back/forward, centred address field
  (≈40 % width: page menu ≡ at left, host centred, reader & reload at right), share, +, tabs;
  row 2 = full-width strip of equal tab segments (favicon + title, active one lighter with ×).
- **Compact tab bar**: one row, tabs as segments, the active segment holds the address (what
  Axion has today).
- **Hide tab bar on scroll:** scrolling the page down collapses the bars into a small pill with
  the host at the top centre; scrolling up (or tapping the pill) brings them back.
- **Colour in tab bar:** the bars take the page's top colour (Axion: `tint_from_page`).
- Page menu (≡): a glass menu dripping out of the button: Website übersetzen …, Zu Lesezeichen
  hinzufügen, Lesezeichen hinzufügen zu …, Auf der Seite suchen, Seitenzoom (100 %) ›, Ablenkende
  Objekte ausblenden, Tab-Aktionen ›, Datenschutz & Sicherheit ›, Mobile Website anfordern.
- Address field focused: a glass dropdown under the field: top hit (highlighted row), then
  "Google-Vorschläge", then "Lesezeichen, Verlauf und Tabs".

## Settings (Safari section)

Grouped lists: white rounded cards on a light grey ground, one row per setting with a separator,
switches at the right, section titles in small grey caps above, explanations in small grey text
below a card.

- **Suchen:** Suchmaschine (Google ›), auch im Modus „Privates Surfen" verwenden,
  Suchmaschinenvorschläge, Safari-Vorschläge, Schnelle Website-Suche, Toptreffer vorab laden.
- **Allgemein:** Autom. ausfüllen ›, Startseitenfavoriten, Favoritenleiste einblenden,
  Favoritenleiste – Darstellung, Links anzeigen wenn Zeiger auf Objekt verweilt, Pop-Ups
  blockieren, Erweiterungen ›, Downloads ›.
- **Tabs:** two illustrated choices *Separate Tableiste* / *Kompakte Tableiste* (radio under each
  picture), Farbe in Tableiste anzeigen, Tableiste beim Scrollen ausblenden, Neue Tabs im
  Hintergrund öffnen, Tabs schließen (Manuell / Nach einem Tag / einer Woche / einem Monat).
- **Profile:** Neues Profil.
- **Datenschutz & Sicherheit:** Cross-Sitetracking verhindern, IP-Adresse verbergen, Touch ID
  zum Entsperren von „Privates Surfen" anfordern, Betrugswarnung, Warnung vor unsicherer
  Verbindung.
- **Verlauf und Websitedaten:** Importieren, Exportieren, Verlauf und Websitedaten löschen.
- **Einstellungen für Websites:** Seitenzoom, Desktop-Website anfordern, Reader, Kamera,
  Mikrofon, Standort.
- **Leseliste:** Automatisch offline sichern. **Erweitert ›**.
