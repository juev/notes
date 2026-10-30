# Notes: import

Status: implemented in the core (`core/src/import/`, stage 2). I1, I7 and I10 are checked by unit tests in their modules, I1–I12 end to end by `core/tests/import.rs`, on backups the tests write. Two devices that import the same backup before they sync hold one text of each note: the first change of an imported text is made the same on both (`i5_two_devices_that_import_the_same_backup_hold_one_text`). Two devices that import different backups of the same account (of different days, or with the files found on one of them only) also agree, on one of the two texts (S3); edits made on the other text before the devices first sync are not shown (`i5_two_devices_that_import_different_backups_still_agree`). File → Import… of the macOS app is checked on a fixture with a file (the report and the attachment in the text). Not done: the manual check on the author's backup (the last scenario below); in I8 an icon that is a symbol name gets the default icon, since the app has no set of symbols yet to map it to; nested tags are imported as flat folders until a backup with them is seen. Sources: the source of Standard Notes (`standardnotes/app`, branch `main` as of 2026-10-28: `packages/models/src/Domain/Syncable/`, `packages/features/src/Domain/Component/NoteType.ts`); the structure of a decrypted backup of the author of the project from 2026-10-29, read by a script that counted types, fields and node kinds without reading the text; the help page of Standard Notes on backups. The content of the author's backup is not part of the repository and is not used in tests.

## Purpose and scope

The first version imports a decrypted Standard Notes backup: notes, tags as folders, smart views as saved filters, and, when their content is given, files as attachments. Import from Markdown folders, Bear and Apple Notes is planned later ([product.md](product.md), "Later").

## What a backup looks like

Observed in the author's backup and in the source; "observed" means seen in the data, "from the source" means read in the code of Standard Notes and not seen in the data.

- The decrypted backup is a zip, or the folder it unpacks to, with the file `Standard Notes Backup and Import File.txt` at the top and a folder `Items/` with one file per item. The top file is a JSON object `{"version":"004","items":[…]}`. Observed.
- An item has `uuid`, `content_type`, `created_at`, `updated_at`, `created_at_timestamp`, `updated_at_timestamp`, `deleted`, `duplicate_of` and `content`. Observed.
- `content_type` values observed: `Note`, `Tag`, `SN|SmartTag`, `SN|File`, `SN|UserPreferences`, `SN|Component`, `SN|ExtensionRepo`. An encrypted backup also has `SN|ItemsKey`, a top-level `keyParams`, and `content` as a string starting with `004:`.
- A note's `content`: `title`, `text`, `noteType`, `editorIdentifier`, `references`, `appData`, `preview_plain`, and the flags `pinned`, `archived`, `trashed`. Observed. From the source: also `starred`, `locked`, `protected`, `spellcheck`, `editorWidth`, `hidePreview`, `conflict_of`. Note types from the source: `plain-text`, `markdown`, `super`, `rich-text`, `code`, `task`, `spreadsheet`, `authentication`; only `super` was observed.
- In a Super note `text` is a Lexical document as JSON: `{"root":{"children":[…]}}`; an empty note has `text` equal to `""`. Observed node types: `paragraph`, `text`, `linebreak`, `tab`, `heading` (`tag` `h1`–`h3`), `list` (`listType` `bullet` or `number`), `listitem`, `quote`, `code` (`language`) with `code-highlight` children, `link` and `autolink` (`url`, `isUnlinked`), `table`, `tablerow`, `tablecell` (`headerState`), `horizontalrule`, `hashtag`, `snfile` (`fileUuid`), `unencrypted-image` (`src`, `alt`). The `format` of a `text` node is a bit mask; observed values are 0, 1, 2, 3, 16 and 17.
- A tag's `content`: `title`, `iconString`, `expanded`, `references` to its notes. Observed; all observed tags are flat.
- A smart view's `content`: `title`, `iconString`, `predicate`: a tree of `{keypath, operator, value}` with `and` and `or`. Observed operators: `and`, `includes`, `=`; from the source also `or`, `!=`, `<`, `>`, `<=`, `>=`, `startsWith`, `in`, `matches`, `not`.
- A file's `content`: `name`, `mimeType`, `decryptedSize`, encryption fields, and `references` to the notes it belongs to with `reference_type` `FileToNote`. Observed. The backup has no file content.

## Requirements

- I1. The importer accepts a decrypted backup as a zip, as its unpacked folder, or as the top `.txt` file alone. The version must be `004`. An encrypted backup (`keyParams` present, or `content` that is a string) is refused with the message to export a decrypted backup from Standard Notes. Another version is refused with a message that names it.
- I2. The import runs in the core, so it gives the same result on macOS, Android and the web server. It writes ordinary changes, which then sync. It is one step of undo (R75 in [product.md](product.md)).
- I3. **Identifiers.** The identifier of every imported entity is computed from the `uuid` of its source item, so importing the same backup twice does not create duplicates: an item whose entity already exists on the device, in the Trash included, is skipped and counted in the report.
- I4. **Notes.** Each `Note` becomes a note: `title` becomes the title, `text` the text (I7 for Super, as it is for `markdown` and `plain-text`). A note of another type gets its `text` as it is and is listed in the report. A note with an empty title and an empty text is skipped and counted.
- I5. **Time.** `created_at` becomes the moment of creation. The fields of an imported note are written with timestamps taken from `updated_at` (the device and counter parts of the timestamp are those of the importing device), so the list of notes shows the moment of the last edit in Standard Notes, not of the import. This is safe because the entities are new (I3): no other device has a value to compete with.
- I6. **Flags.** `pinned` (in `content` or in `appData`), `starred`, `archived` and `locked` become the same flags. `trashed` puts the note into the Trash with the moment of the import, so it stays there for `trash_days` from the import. `protected` is not supported in the first version: the note is imported as an ordinary note and listed in the report. `conflict_of`, `spellcheck`, `editorWidth` and `hidePreview` are ignored.
- I7. **Super to Markdown.** The Lexical tree is converted node by node into the dialect of [editor.md](editor.md):

  | Node | Markdown |
  |---|---|
  | `paragraph` | a paragraph; an empty one is dropped between blocks |
  | `text` | the text, escaped so that characters of Markdown stay literal; bit 1 bold, 2 italic, 4 strikethrough, 16 inline code, 128 highlight; bit 8 (underline), 32 and 64 (subscript, superscript) have no Markdown and are dropped, and the note is listed in the report |
  | `linebreak` | a hard line break; inside a code block, a newline |
  | `tab` | a tab character |
  | `heading` | `#` to `######` by `tag` |
  | `list`, `listitem` | `-` or `1.` items, nested by the nesting of the tree; `listType` `check` becomes a task list item with the state of `checked` |
  | `quote` | `> ` before each line |
  | `code`, `code-highlight` | a fenced block with `language`; the text of the children joined |
  | `link` | `[text](url)` |
  | `autolink` | the address as it is; with `isUnlinked` plain text |
  | `table` and its rows and cells | a GFM table; the cells of the first row, or those with a header state, form the header; several paragraphs in one cell are joined with a space |
  | `horizontalrule` | `---` |
  | `hashtag` | the text as it is, an ordinary word (R23 in [product.md](product.md)) |
  | `snfile` | an attachment link (E30 in [editor.md](editor.md)) to the attachment of I9 |
  | `unencrypted-image` | with an `http` or `https` `src`, a link to the image; otherwise (the observed `../_resources/…`) the text `[Image not imported: <alt or src>]`, listed in the report |
  | any other node | its text content as paragraphs, the node type listed in the report |

  The bits 4 and 128 and the `check` list type are from the Lexical conventions and were not seen in the data.
- I8. **Tags become folders.** Each `Tag` becomes a folder with its title as the name. An `iconString` that is an emoji is kept; a symbol name is mapped to the app's symbol set where an equivalent exists, otherwise the folder gets the default icon. A tag nested under another tag becomes a subfolder; how Standard Notes records the nesting was not observed (all tags of the author are flat), so this rule is completed when a backup with nested tags is available. A note referenced by one tag goes into its folder; a note without tags goes outside any folder. A note referenced by several tags goes into the folder of the tag with the fewest notes in the backup, as the most specific one; with equal numbers, the first by title in alphabetical order. The note gets the other tags as tags; the report lists these notes with the folder chosen. The import can optionally put all folders and notes under one new folder "Standard Notes".
- I9. **Files.** Each `SN|File` that references a note becomes an attachment of that note with its `name`, `mimeType` and `decryptedSize`, when its content is found: the importer is given a folder of files downloaded from Standard Notes in decrypted form, and a file in it matches when its name and size are those of the item. The automatic file backups of the Standard Notes desktop app do not serve: each file there is a folder with `metadata.sn.json` and `file.encrypted`, sealed with the keys of the account (from the source: `packages/files/src/Domain/Device/FileBackupsConstantsV1.ts`). A file referenced by several notes is attached to each. A file with no match, or with several, is not imported: its `snfile` node becomes the text `[File not imported: <name>]`, and the file is listed in the report. A file that references no note is skipped by default; the option "Collect unlinked files" puts them into one note "Files from Standard Notes".
- I10. **Smart views become saved filters.** A predicate whose keypaths and operators all have an equivalent among the conditions of R45 in [product.md](product.md) becomes a saved filter with the same title and icon. `tags includes <title>` becomes "in the folder" of the folder made from that tag (I8); `title =` becomes "the title is", `title includes` "the title contains", `text includes` "the text contains"; `pinned`, `starred`, `archived` with `= true` become the flags; `and` and `or` become "all of" and "any of". Another predicate is not imported and is listed in the report with its title.
- I11. **Ignored items.** `SN|UserPreferences`, `SN|Component`, `SN|ExtensionRepo`, `SN|ItemsKey` and any unknown type are not imported; the report gives their number by type. An item with `deleted` set is skipped.
- I12. **Report.** At the end the app shows: the number of notes, folders, tags, filters and attachments imported; the number skipped as already imported (I3) and as empty (I4); and the lists of I4, I6, I7, I8, I9 and I10, each entry with the title of the note. The report can be copied as text. It is also written to the activity log as numbers only, without titles (R95).

## Verification scenarios

Each requirement is covered by a test in `core/tests/import.rs` named after its identifier, on fixtures written for the tests; no fixture is taken from the author's data.

- I1: an encrypted backup → refused with the message; a version `003` → refused with the version named; a zip, a folder and the `.txt` alone → the same result.
- I3: the same backup imported twice → the second import adds nothing and reports every item as already imported.
- I5: a note with `updated_at` a year ago → the list shows that date; a later edit on another device wins over the imported field.
- I6: a trashed note → in the Trash, with `trash_days` counted from the import; a protected note → an ordinary note in the report.
- I7: a Super document with every node type of the table, the format bits 1, 2, 3, 16 and 17, a nested list, a table with a header row, a code block with a language and line breaks → the expected Markdown, which parses back (E10 in [editor.md](editor.md)) into the same structure; text with `*`, `_`, `#` at the start of a line and a `|` in a table cell stays literal; an underlined word → its text without markup, the note in the report; an `unencrypted-image` with `../_resources/x.jpg` → the placeholder text, in the report.
- I8: a note with the tags "a" (five notes) and "b" (two notes) → in folder "b" with the tag "a", in the report; with the tags "c" and "d" of two notes each → in folder "c" with the tag "d"; a note without tags → outside any folder; with the option on → everything under "Standard Notes".
- I9: a file with a matching name and size → an attachment shown in place of the `snfile` node; two files with the same name and size → not imported, in the report; a file of no note with "Collect unlinked files" on → in the note "Files from Standard Notes".
- I10: `and(tags includes "work", title = "x")` → a filter "all of: in the folder work, the title is x"; a predicate with `matches` → not imported, in the report.
- A manual check on the author's backup on the author's machine, recorded in the status of this document with the numbers of the report and without titles.
