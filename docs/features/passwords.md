# Passwords

Axion has its own password manager. It stores logins, cards, addresses and secure notes in an
encrypted vault on your computer. How the vault is protected is explained in
[Accounts and privacy](accounts-and-privacy.md#how-your-passwords-are-protected).

## Open Passwords

Press **Ctrl+P**, or choose **Passwords** in the sidebar. The first time, Axion asks you to set up a
password for your account (**Set Up Passwords**). After that you enter your account password to
unlock. The list has four kinds of items: **Passwords**, **Cards**, **Addresses** and
**Secure Notes**. Use the search field to find an item, and **+** to add one by hand.

Opening the list asks for the password every time. Once you have unlocked, saving and filling
logins works without asking again until Axion quits or you press **Ctrl+L** (**Lock**).

## Saving a login

After you sign in to a site, Axion asks **Save password?** with the site and user name. Choose
**Save**, or **Not Now**. Turn this off with **Settings > Passwords > Offer to save passwords**.

If you sign in with a different password for a saved account, Axion asks **Change Password?**.
It waits a few seconds first. If the page shows the sign-in form again, the login most likely failed
and Axion does not offer to change anything. A prompt you declined does not come back for the same
login for 30 minutes.

## Filling a login

On a sign-in form Axion offers your saved logins (**Fill with Axion**). If the vault is locked, the
offer says **Unlock to fill**. Turn this off with **Offer to fill saved passwords**.

- Logins for related sites are shared when they belong together. For example, a login saved for
  google.com is also offered on youtube.com and gmail.com. Subdomains of a site always match.
- **Two-step sign-ins** (user name on one page, password on the next) work too: Axion remembers the
  user name from the first step for five minutes, to pair it with the password on the second.

## Generating a password

When you add or edit a login, the **PASSWORD GENERATOR** makes a strong password. Choose its
**Length** (20 by default) and whether it includes **Symbols**, then **Generate**.

## Cards, addresses and notes

- **Card**: cardholder, card number, expiry (MM/YY) and security code. Card details are hidden. Use
  **Show Card Details** or **Copy Card Details**. They ask for the password every time.
- **Address**: full name, street, city, ZIP code, country, email and phone.
- **Secure Notes**: a title and free text.

## Clipboard

Copied passwords and card numbers are removed from the clipboard after a while. Change the delay with
**Settings > Passwords > Clear clipboard after copying** (60 seconds by default, or **Never**).

## Import from other browsers

Passwords can't be read from another browser's files directly (they are encrypted with the desktop
keyring), so they come in as a CSV file. For bookmarks and history see
[Settings > Data & Downloads](../settings.md#import-from-another-browser).

Export your logins from another browser or manager as a CSV file, then in
**Settings > Passwords** type the file's path into **File** and press **Import CSV**. Axion
understands the CSV files of Chrome (and Edge, Brave), Firefox and Bitwarden. Items already in the
vault are not added twice.

## Back up and restore

The vault only opens on the computer it was made on. To keep a safe copy, or to move to another
computer:

1. In **Settings > Passwords**, type a file path in **File** and your account password in
   **Password**.
2. Press **Export Encrypted Copy**. The file is encrypted with the password alone.
3. On the new computer or in another account, press **Restore Axion Export** with the same file and
   password.

Store the export somewhere safe. Anyone who has it and the password can read it.

## If something goes wrong

- **Wrong password** - try again. Axion cannot reset it.
- **The vault came from another device** - the device secret is missing. See
  [Troubleshooting](../troubleshooting.md).
- **Passwords Unavailable** - the vault could not be opened. Check the message and
  [Troubleshooting](../troubleshooting.md).
