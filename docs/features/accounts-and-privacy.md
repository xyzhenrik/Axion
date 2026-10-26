# Accounts and privacy

Everything Axion stores stays on your computer. There is no sign-in with a company account and no
sync. This page explains local accounts, private tabs and how your saved passwords are protected.

## Local accounts

An **account** is a separate profile, like in Chrome. Each account has its own:

- tabs and private tabs
- history, bookmarks and downloads list
- saved passwords, cards and addresses
- website data (cookies, caches, site settings)
- Shields choices per site
- settings (`config.toml`) and shortcuts (`keymap.toml`)

Use accounts to keep work and personal browsing apart, or to share a computer.

### Add, switch and delete

- The **account button** at the bottom of the sidebar shows your picture and name. Click it for a
  menu: your accounts, **Add Account…**, account settings and **Lock**.
- **Add Account…** asks for a name, optionally a photo, and optionally a password. You can set a
  password later in **Settings > Account**.
- Switching accounts **restarts Axion** into the other account, in the same window size and place.
  Chromium can only work with one profile per run.
- **Settings > Account > Delete Account…** removes an account and all its data. The only account
  cannot be deleted. Axion then opens another account.

### Profile picture

In **Settings > Account**, press **Choose Photo…**. Your desktop's file chooser opens. Axion crops
the picture to a square and keeps its own copy in the account, so you can delete the original.
**Remove Photo** goes back to coloured initials. Very large files (over 20 MB) are refused.

## The account password

Each account can have **one password**. It is optional, and it does two jobs:

1. It is the master password of your saved passwords ([Passwords](passwords.md)).
2. It can lock your **private tabs**.

Set it in **Settings > Account > Password**. It needs at least 8 characters. You are asked for it
only when needed: to open locked private tabs, to open **Passwords**, or when saving or filling a
password.

> **Axion cannot recover a forgotten password.** Without it your saved passwords cannot be read. Keep
> an encrypted export somewhere safe (see below).

## Private tabs

Private tabs are kept in a separate profile in memory. Nothing is written to disk: no history,
cookies or cache. Private tabs never show a start page background and cannot add videos to the
Playlist.

- Open one from the **Private** section of the sidebar, or choose **Open in Private Tab** on a link.
- Drag a tab between the normal and the **Private** group in the sidebar to move it. The page reopens
  in the other group.

### Locking private tabs

With **Settings > Privacy > Lock private tabs with the account password** on (the default), the
**Private** section asks for your account password before it opens. This needs an account password;
if you have none, the Privacy page lets you create one right there.

- Enter the password once and private tabs stay open until Axion quits or you lock again.
- **Lock** in the account menu, or **Ctrl+L**, locks private tabs and password filling again.
- Opening the **Passwords** overview, and showing card numbers, ask for the password every time.
- Locked private tabs show a password card. It opens by itself when you switch to Private. In
  `config.toml`, `privacy.prompt_unlock_on_open = false` turns that off.

## Site permissions

Microphone, camera, screen sharing, notifications and the like are asked for per site; your
decisions stay in the account's profile (private tabs: in memory only). See
[Site permissions](site-permissions.md).

## Clearing your traces

- **Settings > Privacy > Clear History…** deletes the browsing history.
- **Settings > Data & Downloads > Clear Browsing Data…** clears history, cookies and site data,
  cache and the downloads list for a time range of your choice.
- **Clear cookies and site data when Axion quits** wipes cookies every time you close the browser.
- The site list on the same page shows which sites store cookies and lets you remove them one by one.

## How your passwords are protected

Your saved passwords are kept in an encrypted vault file inside the account. Each item is encrypted
by itself. To open the vault Axion needs **two things**:

1. your **account password**, and
2. a **device secret** that never leaves this computer.

The device secret is sealed in the computer's **TPM chip** when one is available (`/dev/tpmrm0`;
your user must be in the `tss` group). If not, Axion keeps it in a protected file, `device.key`, in
`~/.local/share/axion`, outside every profile. If a TPM becomes usable later, Axion moves the secret
into it automatically.

What this means for you:

- Copying your profile or the vault file to another computer **does not expose** your passwords,
  even if someone knows the password. The vault says it came from another device.
- **Never lose the device secret.** If `device.tpm` or `device.key` is deleted, or your TPM is
  cleared or the motherboard is replaced, the vault cannot be opened any more, even with the right
  password. Include `~/.local/share/axion/device.key` in your backups (when you have no TPM), and
  keep an encrypted export.
- Do not edit or delete these files while testing. For throwaway test runs set
  `AXION_DEVICE_KEY_DIR` to a temporary folder (see [Configuration files](../configuration.md)).

### Export and restore

Because the vault is tied to your computer, moving to another computer works through an export:

- **Settings > Passwords > Export Encrypted Copy** writes a file that is encrypted with your account
  password alone.
- **Restore Axion Export** reads such a file into the vault of any account, on any device.

Do this before reinstalling your system or changing hardware. See [Passwords](passwords.md).

## Updates and privacy

Axion sends no telemetry. The only network requests it makes on its own are for the Shields filter
lists and, if enabled, one request for the newest release (see [Updates](updates.md)).
