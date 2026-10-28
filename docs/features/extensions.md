# Chrome extensions

Axion runs Chrome extensions (Manifest V3) through Chromium. Content scripts, background service
workers, the `chrome.*` APIs (including `chrome.userScripts`) and options pages work as in Chrome.
The extensions button (the puzzle piece left of Share) manages them. You can install from the
Chrome Web Store, a `.crx` file or an unpacked folder.

## Install from the Chrome Web Store

Open an extension's page on [chromewebstore.google.com](https://chromewebstore.google.com) and press
the store's own button (**Add to Chrome**, **Hinzufügen** …):

1. Axion downloads the extension from Google's update service and checks the package: its signature
   must be valid and made with the key behind the extension's id, so you get the code the id
   names.
2. A window under the extensions button shows the name, the icon, the version and **everything the
   extension can do**: the sites it can read and change (red when that is all websites) and the
   browser features it uses. **Add Extension** installs it, **Cancel** deletes the download.
3. The extension runs after the next restart: the popover shows **Relaunch to Apply Changes**.
   (Chromium loads extensions only at start.)

Extensions you installed show **Remove from Chrome** on their store page. It opens the extension's
details in Axion, where **Remove Extension** (click twice) deletes it.

Only Manifest V3 extensions work: a Manifest V2 extension is refused with a message, because
current Chromium no longer runs them.

Axion contacts only Google's hosts for this (`clients2.google.com` and the `googleusercontent.com`
address it redirects to), over https, and each redirect is checked against that list.
Switch it off with `extensions.web_store = false` (Settings > Privacy > Extensions); the store
then shows a notice instead of opening.

### Updates

Extensions from the store update themselves (`extensions.auto_update`, default on; every
`extensions.update_interval_hours`, default 24). Axion asks the update service for newer versions
of the installed extensions and nothing else, downloads and checks them like a first install,
and keeps the new copy next to the running one. The new version runs after the next restart, and
a toast says so. **Check for Updates** in the popover looks right away.

An update that asks for **more** permissions than the installed version (a new API or site) is not
installed silently: the same window as for a new install opens and shows the update.

## Install from a file or folder

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
for later.

Switch the whole feature off with `[extensions] enabled = false` (Settings › Privacy). Then no
extension runs and the button disappears. Default: on.

## Userscript managers (Tampermonkey, Violentmonkey)

Tampermonkey (id `dhdgffkkebhmkfjojejmpbldmpobfkfo`) installs from the store and runs: its service
worker, content scripts, dashboard and `GM_*` functions work, and a script written in its editor runs
on the pages it matches.

Since Chromium 138 the `chrome.userScripts` API exists only for an extension whose **Allow User
Scripts** switch is on (Chrome shows it on `chrome://extensions`, which Axion does not have).
Axion switches it on for every extension that asks for the `userScripts` permission, before
Chromium starts (`extensions.allow_user_scripts`, default on). You saw that permission
("Run scripts in pages") in the install window. Off: such extensions show their own notice that
user scripts are disabled.

To install a script into Tampermonkey, use its dashboard (**Utilities > Install from URL**, or the
editor). Visiting a `.user.js` address sends Tampermonkey's own installer to tampermonkey.net, which
opens in a tab like any website.

## How it works

`axion-extensions` reads `manifest.json`, works out the id, verifies and unpacks CRX3/CRX2/zip files,
talks to the update service and keeps the extension folders; `axion-store` keeps the list
(`extensions` table; the `source` column says `webstore:<id>` for store installs); `axion-engine`
passes the enabled folders to Chromium as `--load-extension`. CEF 156 has no `LoadExtension` call (it
was removed with the Chrome runtime), so command-line loading is the way.

Why the store's own flow is replaced: its install dialog needs a native window and crashes in
off-screen mode. Axion injects a script into store pages that replaces
`chrome.webstorePrivate.beginInstallWithManifest3` (what the page calls when you press its button) by
a message to the browser, answers `getExtensionStatus` for installed extensions, and replaces
`chrome.management.uninstall` ("Remove from Chrome"). The browser only accepts such a message
from a tab that shows the store, and never installs without its own dialog.

Test aids: `AXION_INSTALL_EXTENSION=<folder or .crx>` installs one before the engine starts;
`AXION_AUTO_CONFIRM_STORE_INSTALL=1` answers the install window with yes (automated runs cannot
click it); `--open=extensions-popover` and `--open=extensions-details` capture the popover.

## Limits

- **`webRequest` and `declarativeNetRequest` are switched off.** An extension that holds them
  makes Chromium stall every navigation to a website in Axion's off-screen browser (the page never
  starts loading; the same happens in Axion without any of this work). Until that is fixed in the
  engine, Axion removes those permissions, and the static rules of `declarative_net_request`, from
  its copy of the manifest (`extensions.block_request_apis`, default on) and the install window says
  "Network request APIs are off in Axion". Ad blockers built on them do not work; Axion's own
  Shields do that job. Tampermonkey and extensions that only use content scripts, storage, tabs and
  the like are not affected.
- **Toolbar popups open in a tab.** Chrome shows an extension's popup in a small window under its
  button; Axion draws the whole window itself and Chromium cannot attach such a popup to it.
  "Open Popup in a Tab" opens the popup page in a normal tab instead (`chrome-extension://<id>/…`).
  It works, but the page has no idea which tab it was "clicked on".
- **No extension buttons in the toolbar** (pin, badge text), no keyboard shortcut commands,
  no extension side panel, no context-menu items drawn by Axion's own menus.
- **`.crx` files you pick yourself are not signature-checked** (you chose the file); store
  downloads are. The package's public key is kept, so the extension has the id it has in Chrome;
  an unpacked folder without a `key` gets an id derived from where Axion stored it.
- **Changes need a restart** (see above). An extension page that is open when Axion starts may
  show "blocked" for a moment; reload it.
- Extensions already installed by the desktop (for example KDE's Plasma browser integration, from
  `/usr/share/chromium/extensions`) are loaded by Chromium too while extensions are on; they are
  not in Axion's list.
- Extensions run with the permissions they declare, minus the ones above. Whether they run in private
  tabs was not verified.
