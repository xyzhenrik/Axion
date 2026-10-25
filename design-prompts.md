# Claude Design prompts — Axion

Paste these into Claude Design (claude.ai) one at a time. Each produces a screen mock-up
we can translate into `axion-ui` components. Start with prompt 0 so all later screens
share one visual language.

---

## 0. Design system

> Create a design system for "Axion", a desktop web browser for Linux inspired by iPadOS 26
> Safari and Apple's Liquid Glass material. Define: glass material tokens (tint light/dark,
> blur radius, refraction strength, rim highlight, shadow), corner radii (capsules for bars,
> 20–28 px for panels), spacing scale, typography (Inter or a similar neutral sans — no
> Apple fonts), an outline icon set style, accent colour (customizable, default a vivid blue),
> light and dark modes, and motion principles (springy, physical, no linear easing).
> Show components: glass capsule bar, glass pill tab (active/inactive/hover), glass button,
> glass popover, sidebar panel, toggle, slider, segmented control, search field.

## 1. Main window — wide layout

> Design the main browser window of Axion at 1600×1000, wide layout. A colourful real-world
> web page fills the entire window edge to edge. Floating over it near the top: a single
> Liquid Glass capsule containing back/forward, the sidebar toggle, a row of tab pills (the
> active one expanded into the address field showing the domain, a shield icon and reload),
> and new-tab / tab-overview buttons. The glass must visibly refract and blur the page behind
> it, with bright rim highlights. On the left, a docked glass sidebar with sections: Tab
> Groups, Bookmarks, History, Now Playing, Shields. Show it in both light and dark mode.

## 2. Responsive variants

> Show the same Axion window at three widths: 1600 px (sidebar docked), 1100 px (sidebar as
> floating glass overlay), 700 px (narrow: tabs collapse into a single compact address
> capsule with a tab count badge). Keep the Liquid Glass look consistent.

## 3. Tab overview

> Design Axion's tab overview: a grid of rounded page-thumbnail cards over a blurred,
> darkened backdrop, each with favicon + title in a small glass label and a close button.
> Top: a glass search field and Tab Group switcher. Bottom: glass capsule with "+",
> "Done" and a group menu. Suggest the zoom transition from the active tab into its card.

## 4. Shields popover

> Design the Shields popover that drops out of the shield icon in Axion's address capsule,
> as if the glass "drips" out of the button. Contents: site name, big toggle (Shields on/off),
> segmented control Off / Standard / Aggressive, counters (ads & trackers blocked, popups
> blocked, redirects blocked, fingerprinting attempts), per-site toggles for third-party
> cookies, scripts, and a "Report broken site" link.

## 5. Media player & fullscreen

> Design Axion's own video player controls, shown on a fullscreen cinematic video frame.
> A floating Liquid Glass control capsule at the bottom: play/pause, ±10 s, a scrubber with
> a thumbnail preview bubble above it, elapsed/remaining time, volume slider, playback speed,
> captions, Picture-in-Picture and exit-fullscreen. A small glass title pill at the top left.
> Also show: the in-page (non-fullscreen) version of the controls, and a Picture-in-Picture
> glass window snapped to the bottom-right corner over a regular web page.

## 6. Settings

> Design Axion's settings as a large glass sheet over the browser, with a left category list
> (General, Appearance, Liquid Glass, Tabs & Memory, Shields, Media, Keyboard, Privacy,
> Advanced). Show the "Tabs & Memory" page: memory budget slider in MB with a live usage bar,
> "freeze background tabs after N min", "discard after N min", exceptions list, and a per-tab
> memory table. Show the "Liquid Glass" page with live preview and sliders for intensity,
> blur, dispersion, tint, motion, plus Reduce Transparency / Reduce Motion toggles.

## 7. Start page

> Design Axion's start page: customizable wallpaper, favourites grid of glass tiles,
> "Frequently visited", a Shields stats card (trackers blocked this week, time and data saved),
> and an "Edit" button to rearrange sections. The address capsule floats at the top.
