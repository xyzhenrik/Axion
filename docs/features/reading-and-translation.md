# Reading and translation

## Reader View

On a page with an article, the **Aa** button appears in the address field. Click it to replace the
page with the article alone: title, author, text and pictures on a clean sheet. Click it again to
open the **Aa menu**: text size, column width, font (Serif, Sans, Mono) and theme (Auto, Light,
Sepia, Dark), and **Exit Reader View**. The shortcut is **Ctrl+Alt+R** (`toggle_reader`).

- The look is remembered for each site (Settings › Reading & Translation turns that off, then it
  is the same everywhere).
- Axion keeps the original page aside, so leaving Reader View is instant and keeps your place.
- The text is found on the page itself with a compact text-density method; no third-party code is
  involved and nothing is sent anywhere.
- Pages without a clear article (start pages, apps, search results) don't show the button.

## Translating pages

When a page is in another language than your target (by default the language of Axion's
interface), the **Translate** button appears in the address field and, once, a small offer opens.
Choose **Translate**; the page's text is replaced piece by piece, what is on screen first.
**Show Original** brings the original text back. **Never for this site** stops the offers for it
(Settings lists these sites; `translation.never_sites`).

### On this computer (default)

Translation runs on your computer with Mozilla's Bergamot engine (WebAssembly) in a hidden page
inside Axion. Nothing is sent to any server while you translate.

- **First use of a language**: Axion tells you how much it needs (about 40 MB per direction,
  plus the 5 MB engine once) and downloads it only after you agree, from Mozilla's Remote
  Settings servers. Every file is checked against its SHA-256 before it is used.
- The files are kept in the profile (`translations/`) and can be removed in Settings.
- Pairs without English (German to French) go through English: both directions are downloaded.
- The engine page closes after a while without work (`translation.engine_idle_seconds`).

**Speed.** The work follows what you see: the text in the window (and one screen around it,
`translation.viewport_margin`) is translated first, in a small first request, and the rest as you
scroll (`translation.scope = "page"` translates everything in the background instead). The popover
shows "N of M" for what has been requested so far, not for the whole page. The engine page runs up to
four workers in parallel (`translation.engine_workers`), compiles the engine once, keeps its
translation service between requests, and is warmed up (started, model loaded) while the offer is
showing (`translation.prewarm`). Equal pieces of text on a page are translated once. Only the top
document of the tab being translated is ever read or changed; frames and other tabs are never touched.
Timings are logged at `info` level (`translation timing: …`, `translation engine: …`).

Licences: the engine glue script is Mozilla's, MPL-2.0 (kept unmodified in its own file); the
models are Mozilla's Firefox Translations models (their repository is MPL-2.0). Axion itself stays MIT.

### Your own server (optional, off by default)

Set `translation.backend = "endpoint"` and `translation.endpoint_url` to a LibreTranslate-compatible
server you run or trust. Then **page text is sent to that server** and nowhere else.

Quality is that of small, fast models: good for understanding a page, not for publishing.
