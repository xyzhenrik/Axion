# Chrome extensions

Axion runs Chrome extensions (Manifest V3) through Chromium. Content scripts, background service
workers, the `chrome.*` APIs and options pages work as in Chrome. The extensions button (the
puzzle piece left of Share) manages them.

## Install, switch off, remove

Click the extensions button:

- **Install from Folder…** takes an unpacked extension (a folder with `manifest.json`).
- **Install from .crx File…** takes a `.crx` or `.zip` package, for example one you downloaded
  or built yourself.
- The toggle of a row switches that extension on or off. **Remove** (in the details) deletes it
  and its files.

Axion copies the extension into your profile (`extensions/` in the account's folder), so
changing or deleting the original does nothing. Installing from the same place again replaces the
copy. **Chromium loads extensions at start, so every change applies after a restart**: the popover
then shows "Relaunch to Apply Changes".

Click a row to see the details and **every permission** the extension declares: the sites it can
read and change (red when that is all websites), the browser features it uses, and what it may ask
for later. Axion shows what the manifest says; it does not narrow it.

Switch the whole feature off with `[extensions] enabled = false` (Settings › Privacy). Then no
extension runs and the button disappears. Default: on.

## Shields and extensions

Shields work as before. Extensions that block or change requests (`declarativeNetRequest`) run
next to them, not instead of them. Axion never loosens a Shields default for an extension. The
extension list is per account.

## Limits

- **Toolbar popups open in a tab.** Chrome shows an extension's popup in a small window under its
  button; Axion draws the whole window itself and Chromium cannot attach such a popup to it.
  "Open Popup in a Tab" opens the popup page in a normal tab instead (`chrome-extension://<id>/…`).
  It works, but the page has no idea which tab it was "clicked on".
- **No extension buttons in the toolbar** (pin, badge text), no keyboard shortcut commands,
  no extension side panel, no context-menu items drawn by Axion's own menus.
- **No Chrome Web Store.** Its "Add to Chrome" dialog needs a native window and crashes in
  off-screen mode, so the store is blocked with a notice. Download a `.crx` yourself and install
  it from the button.
- **`.crx` signatures are not checked** (you chose the file). The package's public key is kept, so
  the extension has the id it has in Chrome; an unpacked folder without a `key` gets an id derived
  from where Axion stored it. Extensions never update themselves.
- **Manifest V2 is refused**: current Chromium no longer runs it.
- **Changes need a restart** (see above). An extension page that is open when Axion starts may
  show "blocked" for a moment; reload it.
- Extensions run with the permissions they declare. Whether they run in private tabs was not
  verified.

## For developers

How it works: `axion-extensions` reads `manifest.json`, works out the id and unpacks CRX3/CRX2/zip
files; `axion-store` keeps the list (`extensions` table); `axion-engine` passes the enabled folders
to Chromium as `--load-extension`. CEF 156 has no `LoadExtension` call (it was removed with the Chrome
runtime), so command-line loading is the way. Test aid: `AXION_INSTALL_EXTENSION=<folder or .crx>`
installs one before the engine starts; `--open=extensions-popover` and `--open=extensions-details`
capture the popover.
