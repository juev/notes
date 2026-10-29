# Notes: design decisions

Status: decisions made on 2026-10-29 in the discussion with the author of the project; nothing is implemented yet. Sources: the request of the author of the project, the design of [juev/lists](https://github.com/juev/lists) (its `docs/architecture.md` and `docs/specs/sync.md`), the source of Standard Notes (`standardnotes/app`, branch `main` as of 2026-10-28), a decrypted Standard Notes backup of the author of the project from 2026-10-29, the documentation of Loro and of jtx Board (links at the end).

## Name

**Notes.** Chosen by the author of the project on 2026-10-29, after the repository. Identifiers: the bundle id and the Android application id are `org.evsyukov.notes`, the Rust crate is `notes-core`, the web server is `notes-web`. A known clash is Apple Notes on macOS; publishing in the app stores is not planned, so the clash is accepted.

## What the app is

A local-first notes app for one person: notes in Markdown with formatting shown in place, attachments that belong to a note, nested folders, tags, and sync through storage the user already owns. It replaces Standard Notes for the author of the project, so importing a Standard Notes backup is part of the first version ([import.md](specs/import.md)).

## Taken from Lists

Notes is built the same way as Lists and starts from a copy of its code. The decision of the author of the project on 2026-10-29: copy, not a shared crate. The two projects then evolve on their own; a fix in the sync code of one is carried over to the other by hand.

| Layer | Language | What lives in it |
|---|---|---|
| Core | Rust | data model, SQLite, merging of changes, text CRDT, sync, push, encryption, Markdown parsing, search, import and export |
| macOS | Swift, SwiftUI and AppKit for the editor | windows, menu bar, Share Extension, the editor on TextKit |
| Android | Kotlin, Jetpack Compose | screens, share target, the editor |
| Web | Rust server `notes-web` on the same core, one HTML page with JavaScript | the phone and any other browser |

The reasons are those of Lists: everything that decides whether the devices converge exists in one copy, in the core; the interface is written in the language of the platform, because the points of quick capture are system APIs. The core reaches the shells through UniFFI, pinned to an exact version, with a narrow boundary: records, enums, synchronous methods of one object, a few stateless functions.

The web interface comes with its own server for the same reason as in Lists: WebDAV servers do not send CORS headers, so a page in the browser cannot talk to the storage directly.

What is copied without changes in behaviour, up to names and paths: the per-field registers with a hybrid logical clock, the change log in a "dumb" storage (WebDAV or a folder), snapshots and their compaction by size, attachment content named by its SHA-256 and its removal through `blob/<hash>/gone`, storage encryption by a passphrase, push between devices through ntfy and UnifiedPush, the choice of side when connecting a storage, backups, the activity log, keeping secrets out of the database. [sync.md](specs/sync.md) describes them for Notes.

## Note text: Loro over Markdown source

In Lists every field is a "last writer wins" register. For a long text this means that of two offline edits of one note one is lost. The author of the project chose a text CRDT on 2026-10-29 instead of the alternatives below.

The body of a note is a [Loro](https://loro.dev) document with one `LoroText` that holds the Markdown source exactly as typed. Two devices that edited different places of the same note while offline get both edits; edits at the same place are both kept, one after the other. All other fields of a note (title, folder, tags, flags) stay LWW registers as in Lists.

Why the Markdown source and not rich text with marks. Decided by the author of the project on 2026-10-29. Loro has a rich text type built on the Peritext criteria, where bold and italic are ranges over plain text, but then lists, tables, code blocks and headings would need a separate block model, and the editors on three platforms would edit that model instead of a string. With the source in `LoroText` the editor edits a string, the Markdown parsing of Lists (ranges in UTF-16 units, painted by the platform) is reused, and export is the text itself. The price: two concurrent edits of markup at the same place can leave stray markers (for example `****`), which the user corrects by hand.

How Loro fits into the change log. A change of the body is not a register value but a Loro update: the bytes that Loro exports for the operations since the previous upload. Applying a Loro update is idempotent and does not depend on order, like applying a register. A snapshot of the storage carries each body as a Loro snapshot. The history kept by Loro is also the history of versions of the note ([product.md](specs/product.md), R40–R43).

Rejected:

- **LWW plus a conflict copy** (as Joplin does). The simplest option; for one person conflicts are rare. Rejected by the author of the project in favour of no loss at all.
- **Three-way merge against the last synced version.** Needs the common ancestor on every device and still falls back to a conflict copy when edits overlap.
- **Automerge.** A text CRDT exists there too; Loro was chosen for its Rust core with a stable format since 1.0 and its rich text and movable tree types, which leave room for later. Not measured against each other on our data.

To check before the core is written (hypotheses, not verified): that `LoroText` accepts positions in UTF-16 units, or the cost of converting them; the growth of a Loro document with full history on the largest note of the Standard Notes backup (273 318 characters of Lexical JSON, so less as Markdown); whether Loro records the wall-clock time of changes, which the version history needs; the Kotlin side: Loro has official bindings for Rust, JavaScript and Swift, and on Android it is reached through our own UniFFI boundary, as everything else in the core.

## Storage: only the change log

The first variant chosen on 2026-10-29 was the change log plus a readable mirror of `.md` files in the storage. The author of the project dropped the mirror the same day, after this comparison:

- A mirror in an encrypted storage is either encrypted, and then useless, or in plain text, and then the encryption means nothing.
- A file that looks editable but whose edits are not synced back and are overwritten by the next write misleads.
- It doubles the storage and the traffic and needs one designated writer.

Reading the notes without the app and opening them in another editor are covered by export to Markdown with attachments and front matter, in a layout that Obsidian opens (R60–R63 in [product.md](specs/product.md)), and by backups. An automatic export into a chosen local folder on a Mac or on the web server is planned after the first version.

## Transports

| Transport | Decision |
|---|---|
| WebDAV | first version, as in Lists |
| Folder (Syncthing, iCloud Drive, a mounted share) | first version, as in Lists |
| S3-compatible storage | after the first version; the change log needs only get, put, list, move and delete |
| CalDAV (VJOURNAL) | not planned for now |

CalDAV for notes was considered at the request of the author of the project. In iCalendar the component for a note is VJOURNAL. By the compatibility list of jtx Board, Nextcloud and Radicale store journals, but have no web interface for them; Posteo does not support VJOURNAL, and Yahoo deletes collections with journals shortly after they are created. No confirmation was found for iCloud and Fastmail. An object is limited to about 10 MB on the servers checked for Lists, so attachments of the size seen in the Standard Notes backup (up to 14 MB) do not fit. What it would give is that notes are visible in jtx Board and Thunderbird; this was judged not worth a second transport in the first version.

Nextcloud Notes API and git were rejected: the first ties the app to one server, the second is heavy to implement on Android and in the web server.

## Markdown and the editor

The note is stored as Markdown; formatting is a matter of display. The core parses the text and returns ranges: what to style, what to hide, what to replace with a bullet, a rule or a checkbox. The platforms only paint. This is the scheme of Lists (`core/src/markdown.rs` there, `pulldown-cmark`). The dialect and the editing rules are in [editor.md](specs/editor.md).

Code blocks are highlighted in the core as well, with `syntect` on its pure-Rust regex engine (decision of the author of the project of 2026-10-29). Rejected: `tree-sitter-highlight`, which parses more precisely but brings a grammar in C for each language, built for every target. Highlighting on each platform was not considered: three highlighters would colour the same block differently.

Within a note the user changes only the kind of text: bold, italic, code, headings, lists and so on. The font, the size, the spacing and the alignment are global settings for all notes (decision of the author of the project on 2026-10-29). They are shared by all devices, except the font size and the column width, which each device keeps for itself, because a phone and a desktop need different values.

The fonts the app offers are bundled with it, so that a note looks the same on macOS, Android and the web. The candidates are fonts under the SIL Open Font License: Literata, Inter, iA Writer Quattro and JetBrains Mono for code. The licence of each is checked when it is added.

## Attachments belong to a note

An attachment has the field `note`. It lives as long as its note: when the note is deleted for good, its attachments are deleted with it, and their content leaves the device and the storage by the mechanism of Lists (content no longer referenced is deleted, `blob/<hash>/gone` is written). In the text an attachment is a link `attachment:<id>`. When the user removes the link from the text, the attachment stays and is shown in the attachments panel of the note as not referenced; it is deleted by hand or together with the note (decision of the author of the project on 2026-10-29). Deleting it automatically when the link disappears was rejected: with a text CRDT one device may remove the link while another keeps it or restores it by undo, and the attachment would be gone for both.

## What was taken from Standard Notes

The author of the project uses Standard Notes now. Its data model (`packages/models/src/Domain/Syncable/` in `standardnotes/app`) and the author's own backup were studied, and the following was taken into the first version: the flags pinned, starred, archived, trashed and locked; the system views (all notes, files, starred, archived, trash, untagged); saved filters in place of smart views; per-folder display settings; the daily mode of a folder; version history; export to Markdown, HTML and PDF with a choice of how attachments are written out.

Left for later: protected notes (opening needs a password or biometrics), the title format for new notes, quick photos into a note, the web clipper. Not taken: separate note types with their own editors (code, spreadsheet, authenticator), publishing to Listed, extensions and themes as plugins, conflict copies (`conflict_of`), which Loro makes unnecessary.

The author's backup shows how Standard Notes is used in practice, and the first version follows it: all 328 notes are in the Super editor; code blocks (232 of them, with a language set in most) and tables (11) are used, so code highlighting and GFM tables are in the first version; the eight tags are flat, almost every note has one tag, and the interface of Standard Notes shows them as folders. So the tags of Standard Notes become folders on import (decision of the author of the project on 2026-10-29).

## Spelling and grammar

Spelling is checked by the system on each platform: `NSSpellChecker` on macOS, the browser on the web, the system spell checker service on Android. Whether the text field of Compose supports the system spell checker was not checked; if it does not, it has to be connected to our editor separately, and this is a risk for the estimate. Grammar is checked later, through a LanguageTool server the user runs; the text goes only to that server.

## Sources

- Lists: <https://github.com/juev/lists>, `docs/architecture.md`, `docs/specs/sync.md`, `docs/specs/product.md`
- Loro: <https://loro.dev/blog/v1.0>, <https://loro.dev/blog/loro-richtext>
- VJOURNAL: <https://jtx.techbee.at/compatibility>, <https://help.nextcloud.com/t/synchronizing-notes-over-caldav-as-vjournal-format/1676>, <https://www.rfc-editor.org/rfc/rfc5545>
- Standard Notes: <https://github.com/standardnotes/app>, <https://standardnotes.com/help/14>, <https://standardnotes.com/features>
