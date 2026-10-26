# Appearance and Liquid Glass

Axion's interface is made of **Liquid Glass**: see-through material that bends, blurs and tints
whatever is behind it. You can keep the full effect, tone it down, or switch to a plain flat look.

![Appearance settings in light mode](../../assets/screenshots/appearance-light.webp)

## Light, dark or automatic

**Settings > Appearance > Appearance** chooses **Automatic** (follows your desktop), **Light** or
**Dark**. The glass, the start page and Axion's own sheets all follow it.

By default the window colour is a calm neutral grey and does not depend on the website you are on.
Two optional switches let the page influence the look:

- **Light or dark chrome to match the page** (Appearance) flips the toolbar between light and dark to
  match the top edge of the page.
- **Colour in tab bar** (Tabs & Memory) tints the window around the page with a colour taken from
  the page, as in Safari. **Page colour** then chooses **Extreme** (the darkest or lightest prominent
  colour) or **Dominant** (the main colour, evened out).

## Glass or flat

**Settings > Appearance > Glass**

| Choice | What you get |
|---|---|
| **Liquid Glass** | The full look: refraction, blur, glass shapes that flow together and split, and springy, physical motion. |
| **Flat** | Solid cards with a thin border and a soft shadow. No refraction, blur, merging or bounce. The layout is tighter and the page fills the window. |

**Flat** skips all the glass drawing. On a weaker graphics card, such as an older integrated one, it
is a lot faster and smoother. Everything else works the same.

![Flat mode in dark](../../assets/screenshots/flat-dark.webp)

## Fine-tune the glass

**Settings > Liquid Glass** has sliders for the material (only used with **Liquid Glass**):

- **Refraction** - how much the glass bends the content behind it.
- **Blur** - how strongly the backdrop is blurred.
- **Colour dispersion** - the thin colour fringes along the rim.
- **Squash when moving** - how far the glass stretches as you drag it.
- **Merge distance** - how close two shapes must be before they flow together.

## Reduce Transparency and Reduce Motion

Two switches on the **Liquid Glass** page make the interface calmer and easier to read:

- **Reduce transparency** - glass becomes opaque surfaces.
- **Reduce motion** - springs, stretching and bouncing become short fades.

They are also offered in the first-start tour. Reduce motion can pause live wallpapers on the start
page; see **Play live wallpapers with Reduce Motion** in [Start page](start-page.md).

## Accent colour

**Accent colour** (Appearance) picks the colour of selections, focus rings and primary buttons.
Buttons in the accent colour are tinted glass, and the text on them switches between light and dark
so it stays readable on any colour. Six colours are offered. You can use any colour in `config.toml`
(`appearance.accent`).

## Icon weight and text size

- **Icon weight** - **Regular**, **Medium** or **Bold** lines for icons. Medium is the default.
- **Text size** - scales all of Axion's own text from 85 to 130 %. It does not zoom web pages; use
  **Ctrl++** and **Ctrl+-** for those.
- **Page corner radius** - how round the page card's corners are (0 to 24 px).

## Layout

- **Tab bar** (Tabs & Memory): **Separate** puts tabs in their own row below the address bar.
  **Compact** puts them in one row where the current tab holds the address.
- **Float sidebar over the page** (Appearance): the sidebar always floats over the page as a glass
  card. Off, it docks beside the page in wide windows and floats in narrow ones.
- **Bookmarks bar** (Tabs & Memory): **Always**, **On Home Page** or **Never**.
- **Show home button** (General): a home button next to back and forward.

![Sidebar floating over a page](../../assets/screenshots/page-dark.webp)

## Language

**Settings > General > Language** switches Axion's own interface between **System**, **English**
and **Deutsch**. Restart Axion for it to apply.
