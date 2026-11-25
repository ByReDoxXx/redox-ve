[Home](Home.md) · [Installation](Installation.md) · [Guests](Guests.md) · [Backups](Backups.md) · [Networking and security](Networking-and-Security.md) · [Command line](Command-Line.md) · [Troubleshooting](Troubleshooting.md)

> Türkçe: [../tr/Ana-Sayfa.md](../tr/Ana-Sayfa.md)

# Interface language

The RedoX VE interface is available in Turkish and English.

- **Per user:** the **EN / TR** button in the top bar. The choice is stored in a browser cookie.
- **Server default:** with `--lang en` on the service line, visitors without the cookie see English (the default is
  Turkish).
- **API:** error messages are English with the `Accept-Language: en` header; the `code` field does not depend on the
  language.

## Coverage

Translated to English: all pages, forms, notices, form errors, alerts and API error messages.

Still Turkish:

- The output and help text of the command line (`redox`).
- Error details that come from inside the drivers (QEMU, LXC, off-site target, chunk store). They may appear in Turkish
  at the end of a task message.
- Command names in the task log are language-neutral anyway (`vm start 103`).

A text without a translation does not break the page: its Turkish version is shown.

## Adding translations

The source text is Turkish. The English table is `crates/redox-server/src/i18n/en.rs`; the key is the Turkish text
itself. In messages with variables the Turkish template uses `{}` and the English one `{0}`, `{1}` in order.
