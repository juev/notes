# Notes: design decisions

Status: decisions made on 2026-10-29 in the discussion with the author of the project; stage 1 of the core is implemented (copied from Lists, notes and folders, note text on Automerge), and stage 2 adds the Markdown dialect and the edits of the editor, code highlighting, import from Standard Notes, search, links between notes, version history and export; stage 3 adds the macOS app; stage 4 adds the Android app; the web interface is not built yet. Sources: the request of the author of the project, the design of [juev/lists](https://github.com/juev/lists) (its `docs/architecture.md` and `docs/specs/sync.md`), the source of Standard Notes (`standardnotes/app`, branch `main` as of 2026-10-28), a decrypted Standard Notes backup of the author of the project from 2026-10-29, the documentation and source of Automerge, Loro and yrs, the RustSec advisory database, the documentation of jtx Board (links at the end).

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
| Android | Kotlin, Jetpack Compose, `EditText` for the editor | screens, share target, the editor |
| Web | Rust server `notes-web` on the same core, one HTML page with JavaScript | the phone and any other browser |

The reasons are those of Lists: everything that decides whether the devices converge exists in one copy, in the core; the interface is written in the language of the platform, because the points of quick capture are system APIs. The core reaches the shells through UniFFI, pinned to an exact version, with a narrow boundary: records, enums, synchronous methods of one object, a few stateless functions.

The web interface comes with its own server for the same reason as in Lists: WebDAV servers do not send CORS headers, so a page in the browser cannot talk to the storage directly.

What is copied without changes in behaviour, up to names and paths: the per-field registers with a hybrid logical clock, the change log in a "dumb" storage (WebDAV or a folder), snapshots and their compaction by size, attachment content named by its SHA-256 and its removal through `blob/<hash>/gone`, storage encryption by a passphrase, push between devices through ntfy and UnifiedPush, the choice of side when connecting a storage, backups, the activity log, keeping secrets out of the database. [sync.md](specs/sync.md) describes them for Notes.

## Note text: Automerge over Markdown source

In Lists every field is a "last writer wins" register. For a long text this means that of two offline edits of one note one is lost. The author of the project chose a text CRDT on 2026-10-29 instead of the alternatives below, first Loro and then, the same day, [Automerge](https://automerge.org) after the comparison in the next section.

The body of a note is an Automerge document with one text object `body` at its root that holds the Markdown source exactly as typed. Two devices that edited different places of the same note while offline get both edits; edits at the same place are both kept, one after the other. All other fields of a note (title, folder, tags, flags) stay LWW registers as in Lists.

Why the Markdown source and not rich text with marks. Decided by the author of the project on 2026-10-29. A rich text model keeps bold and italic as ranges over plain text, but then lists, tables, code blocks and headings would need a separate block model, and the editors on three platforms would edit that model instead of a string. With the source in a text object the editor edits a string, the Markdown parsing of Lists (ranges in UTF-16 units, painted by the platform) is reused, and export is the text itself. The price: two concurrent edits of markup at the same place can leave stray markers (for example `****`), which the user corrects by hand.

How the document fits into the change log. A change of the body is not a register value but the bytes Automerge saves for the changes since the version this device last uploaded (`save_after` with the heads of that version); the reader applies them with `load_incremental`. Applying them is idempotent and does not depend on order, like applying a register. A snapshot of the storage carries each body as a whole saved document (`save`). The history kept by Automerge is also the history of versions of the note ([product.md](specs/product.md), R40–R43).

What the core adds around Automerge, because the library does not do it:

- **Positions.** Documents are created and loaded with the UTF-16 text encoding (`TextEncoding::Utf16CodeUnit`, `LoadOptions::text_encoding`), so the positions of `NSString`, Kotlin and JavaScript go in without conversion. Automerge quietly moves a position that falls inside a surrogate pair; the core checks the position against the text itself and refuses it (S70 in [sync.md](specs/sync.md)).
- **Time of changes.** Automerge stores the time the caller gives a commit (`CommitOptions::with_time`, an `i64` whose unit the caller chooses) and writes 0 without it. The core gives every commit the time of the device clock in milliseconds and never a time below the latest one in the document. Every commit is a separate change; the boundaries of versions (R40) are computed from these times.
- **Version and undo.** There is no revert and no undo manager. Restoring a version (R42) reads the text at that version (`text_at`) and writes it over the current one with `update_text`, which makes ordinary new changes. Undo of the user's own typing in the editor (E25 in [editor.md](specs/editor.md)) is kept by the core, the way Lists keeps undo of actions, and is built with the editor.

Rejected:

- **LWW plus a conflict copy** (as Joplin does). The simplest option; for one person conflicts are rare. Rejected by the author of the project in favour of no loss at all.
- **Three-way merge against the last synced version.** Needs the common ancestor on every device and still falls back to a conflict copy when edits overlap.
- **Loro** and **yrs**: see the comparison below.

### Comparison of text CRDTs

Measured on 2026-10-29 with throwaway crates outside the repository, on one machine, release builds. The texts are the 319 non-empty notes of the author's Standard Notes backup, 748 510 bytes, used on the author's machine and not stored in the repository. Each note was typed character by character, committing every 20 characters two seconds apart and uploading every 30 commits, with 5 % of random corrections and a fixed seed; the correction model is synthetic, and real editing (moving and rewriting whole paragraphs) can grow the history faster. The dependency check is `cargo deny check` with this repository's `deny.toml`, cargo-deny 0.20.2.

| | loro 1.17.1 | automerge 0.12.0 | yrs 0.28.0 |
|---|---|---|---|
| `cargo deny` | advisories fail: 4 unmaintained crates | passes | advisories fail: `smallstr` unmaintained (RUSTSEC-2026-0215) |
| UTF-16 positions | `insert_utf16`, `delete_utf16`, `splice_utf16`; a position inside a surrogate pair is an error | encoding chosen per document; a position inside a pair is moved silently | `OffsetKind::Utf16` per document; a position inside a pair is moved silently |
| Time of changes | recorded by the library in seconds, never goes down; close commits merge | stored as given by the caller, 0 without it, may go down | none |
| Convergence, applying twice | same text | same text | same text |
| Changes with missing dependencies | not checked | kept queued and kept in the saved document (`get_missing_deps`) | kept pending |
| Revert, diff, undo of own edits | `revert_to`, `diff`, `UndoManager` | no revert (done with `text_at` and `update_text`), `diff`, no undo manager | revert by hand, diff only without garbage collection, `UndoManager` when remote changes carry an origin |
| Corpus: saved documents with full history (gzip) | 1.89 MB (1.41 MB) | 0.93 MB (0.93 MB, already compressed) | 1.56 MB (0.87 MB) |
| Corpus: log of updates in base64 | 2.04 MB | 5.92 MB | 3.82 MB |
| Corpus: run time | 0.3 s | 6.7 s | 9.3 s |
| Largest note (92 770 characters): saved size typed once / 5 % / 30 % corrections | 69 / 229 / 810 KB | 10 / 110 / 487 KB | 93 / 230 / 813 KB |
| Largest note: loading | 0.05–0.35 ms | 24–75 ms | 0.07–20 ms |
| Android `aarch64-linux-android`, NDK 28 | builds | builds | builds |
| Version | 1.x, stable format | 0.x; the 1.0 alpha and beta of 2025 were yanked; depends on `hexane 1.0.0-alpha.5` | 0.x, many breaking changes in 0.28 |

Loro was the first choice: it is the fastest, keeps the shortest log, and has everything the notes need. It was dropped on 2026-10-29 by the author of the project because of its dependencies. `cargo deny` stops on four crates marked unmaintained, all archived by their authors, with no maintained version for Loro to move to: `im` 15.1.0 (RUSTSEC-2026-0248), `sized-chunks` 0.6.5 (RUSTSEC-2026-0251), `bitmaps` 2.1.0 (RUSTSEC-2026-0247), `atomic-polyfill` 1.0.3 (RUSTSEC-2023-0089). Two of them also carry reports of unsoundness without a fix, which cargo-deny did not show by default and the local advisory database does: RUSTSEC-2023-0126 (undefined behaviour on insertion into `im::OrdSet`; Loro uses only `im::HashMap` and `im::HashSet`, found by a search of its source) and RUSTSEC-2026-0255 (use-after-free in `sized_chunks::Chunk` when the destructor of an element panics and the panic is caught; release builds of the core abort on panic). Neither is reachable on our path as far as was checked, but these crates sit where the core parses data from the storage, which an unencrypted storage lets anyone with write access shape, and a new fault there would have no fix. Ignoring the advisories was possible and was rejected. yrs was rejected for the same reason (one unmaintained crate), for having no time of changes, and for its log, where every update carries the whole delete set.

The price of Automerge: the log is about three times larger than with Loro, the largest note loads in up to 75 ms, the crate is before 1.0 with an alpha dependency, so its version is pinned exactly, and the core supplies positions inside surrogate pairs, the time of changes, revert and undo itself.

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

Code blocks are highlighted in the core as well, with `syntect` on its pure-Rust regex engine (decision of the author of the project of 2026-10-29). Rejected: `tree-sitter-highlight`, which parses more precisely but brings a grammar in C for each language, built for every target. Highlighting on each platform was not considered: three highlighters would colour the same block differently. The syntaxes built in are those of bat as the crate `two-face` packs them: a set of only the languages of E3 cannot be cut from a built set of syntect, and building one from the sources of the syntaxes was judged not worth it (2026-10-30). syntect needs `bincode` 1.3.3, which RustSec marks unmaintained; the core decodes with it only the syntaxes built into the binary, so the advisory is ignored in `deny.toml` with that reason.

Within a note the user changes only the kind of text: bold, italic, code, headings, lists and so on. The font, the size, the spacing and the alignment are global settings for all notes (decision of the author of the project on 2026-10-29). They are shared by all devices, except the font size and the column width, which each device keeps for itself, because a phone and a desktop need different values.

The fonts the app offers are bundled with it, so that a note looks the same on macOS, Android and the web. The candidates are fonts under the SIL Open Font License: Literata, Inter, iA Writer Quattro and JetBrains Mono for code. The licence of each is checked when it is added.

## Attachments belong to a note

An attachment has the field `note`. It lives as long as its note: when the note is deleted for good, its attachments are deleted with it, and their content leaves the device and the storage by the mechanism of Lists (content no longer referenced is deleted, `blob/<hash>/gone` is written). In the text an attachment is a link `attachment:<id>`. When the user removes the link from the text, the attachment stays and is shown in the attachments panel of the note as not referenced; it is deleted by hand or together with the note (decision of the author of the project on 2026-10-29). Deleting it automatically when the link disappears was rejected: with a text CRDT one device may remove the link while another keeps it or restores it by undo, and the attachment would be gone for both.

## What was taken from Standard Notes

The author of the project uses Standard Notes now. Its data model (`packages/models/src/Domain/Syncable/` in `standardnotes/app`) and the author's own backup were studied, and the following was taken into the first version: the flags pinned, starred, archived, trashed and locked; the system views (all notes, files, starred, archived, trash, untagged); saved filters in place of smart views; per-folder display settings; the daily mode of a folder; version history; export to Markdown, HTML and PDF with a choice of how attachments are written out.

Left for later: protected notes (opening needs a password or biometrics), the title format for new notes, quick photos into a note, the web clipper. Not taken: separate note types with their own editors (code, spreadsheet, authenticator), publishing to Listed, extensions and themes as plugins, conflict copies (`conflict_of`), which the merge of the text by Automerge makes unnecessary.

The author's backup shows how Standard Notes is used in practice, and the first version follows it: all 328 notes are in the Super editor; code blocks (232 of them, with a language set in most) and tables (11) are used, so code highlighting and GFM tables are in the first version; the eight tags are flat, almost every note has one tag, and the interface of Standard Notes shows them as folders. So the tags of Standard Notes become folders on import (decision of the author of the project on 2026-10-29).

## The macOS app

`apple/` is built from parts of the app of Lists: the project, keychain, sync, backups and the text view that paints the layout of the core. The window is new, since Lists has no column for one open document. Three things differ from Lists on purpose.

- The text goes to the core as splices joined by word (`EditCoalescer`), not as a whole string when editing ends: two devices must merge their typing through the CRDT, and the whole string would turn an edit from another device into a deletion. A splice opens the Automerge document, tens of milliseconds on a large note, so one keystroke is not one call.
- Typing has an undo history of its own (`TypingHistory`). The undo of `NSTextView` keeps positions that an edit from another device makes wrong, and E25 asks that such an edit stays when the user's typing is undone.
- An attachment in the text stays TextKit 1: the first character of its link is a control glyph laid out as a box of the size of the attachment, the rest of the link has no glyphs, the paragraph gets a minimum line height, and the view of the attachment lies over the box. TextKit 2 was not taken, because all of the hiding of markup works on glyphs.

## The Android app

`android/` is built from parts of the app of Lists: the build, secrets in the Keystore, background work, backups, push and the viewer of attachments. The screens are new. The editor differs from Lists on purpose.

- The editor is a subclass of `EditText` inside a Compose screen, not `MarkdownField` of Lists. An experiment on an emulator with API 36 and Gboard on 2026-10-30, with Compose foundation 1.12.1, decided it: `BasicTextField` showed no spelling underlines in either of its two forms and takes no inline content, while `EditText` underlined words, kept markup hidden and laid out a picture in the text with spans. An `EditText` checks only the paragraphs edited since it was opened, and the app covers the rest with a session of its own (E50).
- The text goes to the core as splices joined by word, as on macOS, and typing has the same undo history of its own. Lists saves the whole string when the field loses focus, which would turn an edit from another device into a deletion.
- The cost of a keystroke in an `EditText` grows with the number of spans around it, and one span over the whole text makes the view lay out all of it at every key. Three things keep it down: spans change by difference, a long note has them only within 1 500 characters of the screen, and a filter (`MovedSpanFilter`) keeps from the layout the spans an edit only moved. On the emulator a keystroke went from 95 ms at 37 690 characters to 2.3 ms, and takes 3.7 ms at 152 524 characters, with about 4 and 11 ms for painting. The layout of the core runs off the main thread: under 4 ms at 37 724 characters. Not measured on a phone.

## Spelling and grammar

Spelling is checked by the system on each platform: `NSSpellChecker` on macOS, the browser on the web, the system spell checker service on Android. The text field of Compose does not use the spell checker service at all, which is one of the reasons the Android editor is an `EditText` (see "The Android app"). Grammar is checked later, through a LanguageTool server the user runs; the text goes only to that server.

## Sources

- Lists: <https://github.com/juev/lists>, `docs/architecture.md`, `docs/specs/sync.md`, `docs/specs/product.md`
- Text CRDTs: <https://automerge.org>, <https://docs.rs/automerge/0.12.0>, <https://loro.dev/blog/v1.0>, <https://loro.dev/blog/loro-richtext>, <https://docs.rs/yrs/0.28.0>
- Advisories: <https://rustsec.org/advisories/RUSTSEC-2026-0248>, <https://rustsec.org/advisories/RUSTSEC-2026-0251>, <https://rustsec.org/advisories/RUSTSEC-2026-0247>, <https://rustsec.org/advisories/RUSTSEC-2023-0089>, <https://rustsec.org/advisories/RUSTSEC-2023-0126>, <https://rustsec.org/advisories/RUSTSEC-2026-0255>, <https://rustsec.org/advisories/RUSTSEC-2026-0215>
- VJOURNAL: <https://jtx.techbee.at/compatibility>, <https://help.nextcloud.com/t/synchronizing-notes-over-caldav-as-vjournal-format/1676>, <https://www.rfc-editor.org/rfc/rfc5545>
- Standard Notes: <https://github.com/standardnotes/app>, <https://standardnotes.com/help/14>, <https://standardnotes.com/features>
