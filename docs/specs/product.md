# Notes: features and interface

Status: specification, nothing is implemented. Sources: the request of the author of the project and the discussion of 2026-10-29; the data model of Standard Notes and the author's backup ([architecture.md](../architecture.md)); `docs/specs/product.md` of Lists for the parts taken from it.

## Purpose and scope

Notes is a notes app for one person, with native apps for macOS and Android and a self-hosted web interface. Notes are written in Markdown, and formatting is shown in place. Attachments belong to a note and are shown inside it. Everything works offline; sync goes through a WebDAV folder or a plain directory ([sync.md](sync.md)).

The first version covers what the author of the project uses in Standard Notes today, plus attachments shown inline, typography settings and version history. Out of scope for the first version: iOS (the phone is served by the web interface), shared notes and several users, publishing notes, protected notes, the web clipper, CalDAV, grammar checking, import from apps other than Standard Notes. They are listed under "Later" at the end.

The interface is in English with a Russian localization and follows the system language.

## Requirements

### Notes

- R1. A note has a title, a text in Markdown, a folder or none, tags, the flags pinned, starred, archived and locked, the moment it was created and the moment it was last edited. The title is a separate field above the text.
- R2. A new note is created in the folder that is open, or, from a view that is not a folder, in the folder set by `new_note_folder` (S12 in [sync.md](sync.md)): outside any folder (the default), the last folder a note was created in, or a chosen folder. The cursor is placed in the title; Return moves it to the text.
- R3. A note without a title is shown in the lists by the first line of its text; a note without a title and text is shown as "Untitled". An empty note that is left without any edit is deleted without going to the Trash.
- R4. Edits are saved as they are typed; there is no Save button. A note can be duplicated: the copy gets the title with " (copy)", the same folder, tags and text, and copies of the attachments that refer to the same content.
- R5. The list of notes shows for each note the title, a preview of the first lines of text without markup, the date of the last edit and icons of the flags and attachments; the folder setting R16 can hide the preview and the date.

### Folders

- R10. Folders are nested to any depth. A note lies in one folder or in none. A folder has a name and an icon: a symbol from the app's set or an emoji.
- R11. Notes and folders are moved by dragging and through the menu "Move to…". A folder cannot be moved into itself or into its own subfolder.
- R12. The sidebar shows the tree of folders with the number of notes in each, counting the notes of its subfolders. A folder can be collapsed; whether it is collapsed is a setting of the device.
- R13. "No folder" in the sidebar lists the notes that lie outside any folder.
- R14. Deleting a folder moves the folder, its subfolders and all their notes to the Trash, after a confirmation that names the number of notes. Restoring a note puts it back into its folder and restores the folders on the way if they are in the Trash (S10 in [sync.md](sync.md)).
- R15. A folder name is not empty and not longer than 200 characters; two folders in one parent may have the same name.
- R16. Each folder has its own display settings: sort by last edit (the default), by creation or by title, ascending or descending; show the preview; show the date. Pinned notes come first under any sort. The system views (R44) have the same settings, stored on the device.
- R17. **Daily mode.** A folder can be switched to daily mode. In such a folder the list of notes is grouped by the day of creation, newest first, under date headers. "New note" in it creates a note whose title is today's date in the long form of the interface language (for example "29 October 2026" or "29 октября 2026"); the title can be changed. Several notes per day are allowed.

### Tags

- R20. Tags are shown as chips under the title of a note. A tag is added by typing in the field after the chips, with completion from the existing tags; Return or a comma ends it. A tag is removed by its chip.
- R21. A tag name is one or more words without the characters `,` and `#`, of up to 196 bytes of UTF-8 (S5 in [sync.md](sync.md)); leading and trailing spaces are dropped. Tags are compared ignoring case: "Work" and "work" are one tag, and the spelling first written is kept.
- R22. The sidebar lists the tags with the number of notes; choosing a tag lists its notes from all folders. A tag can be renamed and deleted for all notes at once; renaming into an existing tag merges the two.
- R23. A word with `#` in the text of a note is ordinary text, not a tag.

### Flags

- R25. **Pinned.** A pinned note comes first in its folder and in every view, under any sort.
- R26. **Starred.** A starred note is listed in the view "Starred". Starring does not change the order.
- R27. **Archived.** An archived note is not shown in its folder, in "All notes", in tag lists or in saved filters, unless the filter asks for archived notes. It is listed in "Archived" and found by search with the switch "Include archived".
- R28. **Locked.** A locked note opens read-only: the text, the title, the tags and the attachments cannot be changed, and the editor shows a lock with the action "Unlock". Moving, flagging and deleting a locked note stays possible. Edits that come from other devices are applied. Locking is a field and syncs.

### Attachments

- R30. A file is attached through the menu or the toolbar, by dragging it into the note, by pasting it from the clipboard, and through the share sheet of the system (R85). It is inserted at the cursor as a link `attachment:<id>` ([editor.md](editor.md), E30).
- R31. Images, video, audio and PDF are shown inside the note, in place of the link. An image takes the width of the column at most; a tap opens it full-screen. Video and audio get a player. A PDF shows its first page with the number of pages; a tap opens the whole document inside the app, with the option to open it in the system app.
- R32. Any other file is shown as a chip with an icon of its type, its name and its size; a tap opens it with the system app (macOS: a copy under the original name in a temporary folder, as in Lists; Android: through `FileProvider`; web: a download).
- R33. Each note has an attachments panel that lists all its attachments. An attachment whose link is no longer in the text is marked "Not in the text" and has the actions "Insert at cursor" and "Delete". An attachment is deleted only by hand from this panel or together with its note.
- R34. Moving a note to the Trash keeps its attachments; restoring it brings them back. Deleting a note for good deletes its attachments from the device and from the storage (S7, S26, S27 in [sync.md](sync.md)).
- R35. An attachment whose content has not reached the device yet is shown in its place as pending; a tap downloads it ahead of the queue and opens it. A failure is shown in the same place and does not prevent trying again. The number of attachments waiting for transfer is shown by the sync status. As R76 of Lists.
- R36. One file is at most 512 MiB and the attachments of one note at most 2 GiB together; a larger file is refused with a message that names the limit. On the web server the limits are set by `NOTES_MAX_FILE_MB` and `NOTES_MAX_NOTE_MB`. As in Lists.
- R37. The view "Files" lists all attachments of notes that are not in the Trash, newest first, with the name, the type, the size and the title of the note. A tap opens the file; the action "Show in note" opens the note at the place of the link.

### Version history

- R40. Every note has a history of versions of its text. A version is the state of the text after a period of edits followed by five minutes without edits, on any device; the list shows the time, the device and how many characters were added and removed.
- R41. A version opens read-only, with the changes against the current text highlighted.
- R42. "Restore this version" writes into the current text the edit that turns it into that version. It is an ordinary edit: it syncs, it can be undone, and it does not remove later versions from the history.
- R43. The history covers the text only. The title, the folder, the tags and the flags keep only their current value.

### Views, filters and search

- R44. The sidebar has the system views "All notes", "Files" (R37), "Starred", "Archived", "Trash" and "Untagged" (notes without tags), then saved filters, then the folders (R12), then the tags (R22). "All notes" lists every note that is not archived and not in the Trash.
- R45. A saved filter has a name, an icon and conditions joined by "all of" or "any of": in a folder (with or without its subfolders), has a tag, has no tag, has none of the tags, is pinned, is starred, is archived, has attachments, the title is, the title contains, the text contains, created or edited before, after or within the last N days. Filters are listed in the sidebar with the number of notes and sync.
- R46. **Trash.** The Trash lists deleted notes with the number of days left before they are deleted for good. A note can be restored or deleted for good; "Empty Trash" deletes all of them after a confirmation. A note is deleted for good automatically after `trash_days` days, 30 by default; the number is set in the settings and applies on all devices (S8, S12 in [sync.md](sync.md)).
- R47. **Search** finds notes by the title, the text, the tags and the names of attachments. It finds any part of a word, ignores case, and treats "е" and "ё" as the same letter. Results are listed by relevance, with the found part highlighted in the preview; archived notes and the Trash are included only on request. The text is searched without its markup: a search for "word" finds `**word**`.
- R48. **Find in a note:** ⌘F on macOS, the menu on Android and the web; with replace. Matches are highlighted, and the next and previous are reached by Return and Shift-Return.

### Links between notes

- R50. A link to another note is typed as `[[` followed by a part of its title: the editor shows the matching notes, and the choice inserts a link that keeps working after the note is renamed or moved ([editor.md](editor.md), E31). A tap on the link opens the note.
- R51. A note shows the list of notes that link to it ("Linked from") under the text.
- R52. A link to a note that is in the Trash or deleted for good is shown as broken; a tap on a link to a note in the Trash offers to restore it.
- R53. "New note from selection" moves the selected text into a new note in the same folder, with the first line as its title, and leaves a link to it in place of the selection.

### Export

- R60. One note, a folder with its subfolders, or all notes can be exported to Markdown. Each note is a file `<title>.md` with front matter in YAML: the title, the moments of creation and of the last edit, the tags, the flags that are set and the identifier. Folders become directories. Attachments are written into `_attachments/` next to the notes, links to them become relative paths, and links between notes become `[[<title>]]` links that Obsidian follows. Characters that are not allowed in file names are replaced, and two notes with the same title in one folder get a number.
- R61. A note can be exported to HTML and to PDF with the typography of the app ([editor.md](editor.md), E40–E48). For PDF the page size is A4 or Letter. For HTML the attachments are either embedded in the file or written next to it.
- R62. Everything can be exported to JSON in a documented format with all fields, the text of every note and, optionally, the attachments, as a way to leave the app.
- R63. Export does not change the notes and works offline.

### Import

- R65. A Standard Notes backup can be imported: File → Import… on macOS, the settings on Android, the sidebar in the web interface. The rules are in [import.md](import.md).

### Backups

- R70. The macOS and Android apps keep automatic local backups: a zip with all fields, the text of every note with its history and the attachment content. The schedule is daily (the default), weekly or never; the number kept is set, 10 by default; a backup is made at the first opportunity after its time. As R87–R88 of Lists.
- R71. With sync through WebDAV or a folder, the backups also go into the storage without attachment content (S32 in [sync.md](sync.md)), and the same number is kept there.
- R72. A backup can be restored from the list of backups or from a file. Restoring replaces the data on the device; with sync on, the app first asks what to do with the storage (S38 in [sync.md](sync.md)).

### Undo

- R75. Actions on notes and folders (moving, flagging, deleting, restoring, renaming) can be undone and redone: ⌘Z and ⇧⌘Z on macOS, a button and the system "undo" on Android, a button on the web. The history of actions lives in the core, as in Lists (`undo.rs` there). Undo of typing in the editor is described in [editor.md](editor.md), E25.

### Sync and settings

- R80. The settings: appearance ([editor.md](editor.md), E40–E48), the theme (system, light, dark), spelling (E50), the days in the Trash (R46), the folder for new notes (R2), sync and push ([sync.md](sync.md)), backups (R70), the level of the activity log (R95). The interface language follows the system.
- R81. The sync status is visible in the sidebar: synced, syncing, the number of attachments waiting for transfer, or the error with its text. A tap starts a sync.
- R82. The storage password, the encryption passphrase and the push token are kept in the macOS keychain and, on Android, encrypted with a key from the Android Keystore; never in the database or the storage. As in Lists.

### Quick capture

- R85. **macOS:** the Share Extension takes text, a link, images and files from other apps and creates a new note with them in the folder chosen in the sheet; ⌘N in the app creates a note.
- R86. **Android:** the app is a share target for text, links, images and files, which become a new note; a launcher shortcut and a Quick Settings tile create a new note.

### The web interface

- R90. The web server `notes-web` is the same core with its own copy of the data, syncing with the storage like any device. It serves one page with the same views and editor as the apps, adapted to a phone screen. Sign-in is by a password set in `NOTES_WEB_PASSWORD`, and optionally by OpenID Connect, as in Lists.
- R91. On the web, attachments are streamed from the server with byte ranges, so video and large PDFs open without loading the whole file.
- R92. The page can be added to the Home Screen of a phone; the service worker keeps the page itself for when the server is not reachable, and says so instead of showing stale notes as editable.

### Activity log

- R95. The core writes an activity log `log.txt` in the data folder, limited to 1 MB with the previous file kept as `log.1.txt`: sync, push, backups, import, errors. It gets outcomes and numbers, not the text or the titles of notes. As R102 of Lists.

## Interface

- macOS: three columns (the sidebar of R44, the list of notes, the editor) with a toolbar for formatting, attachments and flags; a note can also be opened in its own window. Keyboard shortcuts for formatting are in [editor.md](editor.md).
- Android: a panel with the sidebar, the list of notes, the editor full-screen; formatting from a toolbar above the keyboard.
- Web: the macOS layout on a wide screen, the Android layout on a phone.

## Verification scenarios

Each requirement is covered by a test named after its identifier where the behaviour lives in the core (`r14_*` in `core/tests/`), and checked in the apps by hand where it is in the interface; the specification records which checks were made. Examples of the scenarios:

- R14: a folder with a subfolder and three notes is deleted → the confirmation names three notes; all of them and both folders are in the Trash; restoring one note restores both folders, and the other two notes stay in the Trash.
- R17: a folder in daily mode → "New note" gives a note titled with today's date; on the next day the list has two date headers.
- R21: tags "Work" and "work" on two notes → one tag with two notes in the sidebar.
- R28: a locked note → typing does nothing, "Unlock" makes it editable; an edit from another device appears in a locked note.
- R33: the link to an image is deleted from the text → the image is in the attachments panel as "Not in the text"; "Insert at cursor" brings it back; "Delete" removes it, and after the note is deleted for good its content is gone from the storage.
- R40–R42: edits at 10:00 and 10:20 → two versions; restoring the 10:00 version gives its text, and the 10:20 version is still in the history; on another device the text is the restored one.
- R46: a note deleted 31 days ago with `trash_days` 30 → it is deleted for good on the next start, with its attachments.
- R47: a note with `**слёзы**` → found by "слезы" and by "лёз".
- R50: a link to a note, then the note is renamed and moved → the link opens it; the note shows the first one under "Linked from".
- R60: a folder with a subfolder, a note with an image and a link to another note → the export opens in Obsidian with the folders, the image shown and the link working.

## Later

Planned after the first version, in no fixed order: protected notes (opening needs biometrics or a password); grammar checking through a LanguageTool server; automatic export to a local folder; an S3 transport; import from Markdown folders (Obsidian), Bear and Apple Notes; a title format for new notes; quick photos into a note on Android; the web clipper; an API with tokens for scripts, as in Lists.

## Open questions

- R17: whether "New note" in a daily folder should open today's note when it already exists, instead of creating a second one.
- R21: whether a tag name may contain spaces. The specification allows them; Standard Notes allows them too, while inline `#tags` in other apps do not.
- R40: whether five minutes of quiet is the right boundary of a version.
