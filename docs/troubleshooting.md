# Troubleshooting

## Chrome Web Store: "Add to Chrome" does nothing

Installing extensions from the Chrome Web Store is not supported; Axion does not load
extensions yet. The extension system is switched off (`[extensions] enabled = false`, the
default), so the Web Store's install button fails harmlessly. Turning it on is not recommended:
Chromium's install dialog has no window to attach to in Axion and would crash the browser.
