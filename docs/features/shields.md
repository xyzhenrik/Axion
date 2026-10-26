# Shields

Shields is Axion's built-in blocker. It stops ads, trackers, unwanted popups and sneaky redirects
before they load. Nothing to install, and it is on by default.

## The Shields button

Next to the address field is a shield. Click it to see what was blocked on the current page and to
set the level for this site.

| Level | What it blocks |
|---|---|
| **Off** | Nothing on this site. |
| **Standard** | Third-party ads and trackers, ad popups and ad redirects. |
| **Aggressive** | Also first-party ads, every redirect to another site that happens without a click, and `utm_` tracking parameters. |

Changing the level reloads the page. A site you never set uses the default level from
**Settings > Shields**, **Shields on sites you haven't set** (**Standard** unless you change it).
Change the default and every site without its own level follows.

## Per-site exceptions

If a site breaks, do not weaken Shields everywhere. Open the Shields button on that site and lower
the level for it only (for example to **Off**). Axion remembers the choice for that site, and every
other site stays protected. Set it back the same way.

## What Shields does

These switches are in **Settings > Shields**. Turn any of them off if you do not want it.

| Setting | What it does |
|---|---|
| **Hide empty ad spaces** | Hides the gaps and "Advertisement" boxes that blocked ads leave behind. |
| **Block popups opened without a click** | Blocks new tabs and windows that a page opens without you clicking or pressing a key. |
| **Block redirects to ad and tracker sites** | Blocks automatic jumps to sites on the block lists, including "tab-under" tricks. |
| **Remove tracking parameters from links** | Removes click-tracking parts such as `fbclid` and `gclid` from addresses. |
| **Block third-party cookies** | Stops sites from storing cookies for other sites. |

## Filter lists

The rules come from public filter lists. Axion starts with five:

- EasyList and EasyPrivacy
- uBlock Origin's filters and privacy lists
- Peter Lowe's ad server list

The lists refresh by themselves every 96 hours. **Settings > Shields > Filter lists** shows how many
lists are active and has an **Update Now** button. To use other lists, edit `shields.lists` in
`config.toml` (see [Configuration files](../configuration.md)); the lists use the usual Adblock Plus
and uBlock Origin syntax.

## Statistics

Blocked trackers and ads are counted. The start page can show the totals, plus an estimate of the
bandwidth and time saved (turn it off with **Shields statistics**). See [Start page](start-page.md).

## Private tabs

Shields work the same in private tabs.
