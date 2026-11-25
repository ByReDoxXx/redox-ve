[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Interface language

The RedoX VE interface is available in Turkish and English.

- **Per user:** the **EN / TR** button in the top bar. The choice is stored in a browser cookie.
- **Server default:** with `--lang en` on the service line, visitors without the cookie see English (the default is
  Turkish).
- **API:** error messages are English with the `Accept-Language: en` header; the `code` field does not depend on the
  language.
- **Command line:** `REDOX_LANG=en redox vm list`. The help text, output and error messages are English.
  `redox serve --lang en` also makes the server's own startup messages, alert notifications and the backup report
  e-mail English.

## Coverage

Translated to English: all pages, forms, notices, form errors, alerts, API error messages, the command line, and the
error messages of the drivers (QEMU, LXC, off-site target, chunk store, e-mail, single sign-on).

Not translated:

- Command names in the task log are language-neutral (`vm start 103`).
- Output of external programs (error lines from QEMU, `qemu-img`, `nft`) is shown as it is.
- The `-kopya` suffix of a standby copy's name and the `tasindi-` / `kopya-` tags are data, not text.

A text without a translation does not break the page: its Turkish version is shown.

## Adding translations

The source text is Turkish. The English table is `crates/redox-server/src/i18n/en.rs`; the key is the Turkish text
itself. In messages with variables the Turkish template uses `{}` and the English one `{0}`, `{1}` in order.
