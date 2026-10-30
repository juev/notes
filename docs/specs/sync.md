# Notes: data and sync

Status: stage 1 of the core is implemented in `core/`. The mechanism is copied from Lists ([architecture.md](../architecture.md)) and checked again by the tests of Notes; the note body is an Automerge 0.12.0 document (S3, S4, S20–S22, S70), chosen on 2026-10-29 after a comparison with Loro and yrs by throwaway prototypes ([architecture.md](../architecture.md)). Every requirement except S40 and S44 has a test in `core/tests/` named after it; S71, the name of a device, came with stage 2 and is checked in `core/tests/history.rs`. Not in the core: S40, which is the behaviour of the editor (the core reports that a body changed; the macOS app applies the change as an edit that keeps the cursor, checked by a debug scenario); S44 and the timers, foreground and folder-watch triggers of S39, which belong to the apps (the core gives `sync_now` and the push wait). Sources: the request of the author of the project of 2026-10-29; `docs/specs/sync.md` of Lists.

## Purpose and scope

This document describes how the data is laid out on a device, how edits are merged and what lies in the remote storage. The project has no server of its own: the storage is a WebDAV directory or a local folder, and all it has to do is read, write, list, move and delete files.

Out of scope for the first version: CalDAV and S3 transports, shared access for several people, real-time co-editing of one note, encryption of the data folder on the device.

## Model

Entities: `note`, `folder`, `attachment`, `filter`, `settings`, `blob`. The identifier is a UUID version 7 created on the device, except `settings` (the fixed identifier `app`) and `blob` (the SHA-256 of the content).

Every field except the body of a note is stored as a separate row: kind, identifier, field name, value in JSON, timestamp. The body of a note is an Automerge document stored next to its rows (S3). The tables that the interface queries (`notes`, `folders`, `note_tags`, `attachments`, `filters`, the full-text index) are derived from the rows and the bodies and are rebuilt for an entity on every change.

Note fields: `title`, `body` (Automerge, S3), `folder`, `created`, `pinned`, `starred`, `archived`, `locked`, `deleted`, `purged`, and one field `tag:<name>` per tag with the value `true` or `false`.
Folder fields: `name`, `icon`, `parent`, `pos`, `daily`, `sort`, `show_preview`, `show_date`, `deleted`.
Attachment fields: `note`, `name`, `mime`, `size`, `sha256`, `deleted`.
Filter fields: `name`, `icon`, `query`, `pos`, `deleted`.
Settings fields: listed in S12.
The `blob` field: `gone`, the content with this hash is deleted from the storage (S27).

The moment a note was last edited is not a field: it is the greatest timestamp among its rows and the last change of its body, computed on the device.

## Requirements

### Merge

- S1. A timestamp is a hybrid logical clock: milliseconds, a counter and the device identifier, written as a fixed-width string, so that the strings compare as timestamps. A device that receives a foreign timestamp from the future moves its own clock past it. As in Lists.
- S2. For each triple "kind, identifier, field" except `note/<id>/body` the value with the greater timestamp wins. Applying a change is idempotent, and the result does not depend on the order of application. As in Lists.
- S3. The body of a note is an Automerge document with one text object `body` at its root that holds the Markdown source as typed. The device that creates the note creates the text object; other devices receive it with the first body change (S20). A local edit is applied to the document as insertions and deletions at positions; the change that goes to the storage is the bytes Automerge saves for the changes since the version this device last uploaded for this note. Applying them is idempotent and does not depend on order: two devices that received the same changes in any order have the same text. Edits made offline on two devices at different places of the text are both kept; edits at the same place are both kept, one after the other.
- S70. Positions in the body are counted in UTF-16 units, as the platforms count them: every document is created and loaded with the UTF-16 text encoding. A position inside a surrogate pair or past the end of the text is refused by the core with an error; Automerge itself would move it silently. Every commit of the body carries the time of the device clock in milliseconds, never below the greatest time already in the document, so that the version history (R40 in [product.md](product.md)) can find its boundaries; every commit is a separate change.
- S4. Each device has an Automerge actor identifier, 16 random bytes chosen when the device identifier is created. A new device identifier (S36, S39) gets a new actor identifier, so that two writers never share one.
- S71. A device may have a name, which the app sets (the name of the computer or the model of the phone): the register `device/<actor>/name`, keyed by the actor identifier of S4 in hexadecimal. It syncs like any register, so the history of versions (R40 in [product.md](product.md)) names the device of an edit on every device. A new actor identifier starts without a name until the app sets it again.
- S5. A register key — kind, identifier and field name — is non-empty and no longer than 200 bytes of UTF-8; a change with any other key is not applied. A tag is the field `tag:<name>`, so a tag name is no longer than 196 bytes. As in Lists.
- S6. Moving a note to the Trash is the `deleted` field with the moment of deletion; restoring writes an empty value. A note deleted on one device and edited on another stays deleted, and the edit is kept and visible after restoring.
- S7. Deleting for good is the `purged` field. It merges by S2, and there is no reverse operation: a note that one device purged and another restored while offline stays purged. A purged note is not shown anywhere; the device drops its Automerge document and the full-text index entry, keeps its rows, and does not write the body into snapshots. Its attachments are no longer needed (S26).
- S8. The Trash is emptied by age: a note that has been in the Trash longer than `trash_days` (S12) is purged by any device that notices it, at the end of a pass that read the whole storage or on start without sync. Two devices doing it write the same field; the timestamps are settled by S2.
- S9. Folders form a tree by the `parent` field. If after a merge the chain of parents forms a cycle, every device breaks it the same way: for the folder in the cycle whose `parent` field has the greatest timestamp, the parent counts as empty. The stored fields are not rewritten on merge; the break is written into the field on the next local move of any folder. As S5 of Lists.
- S10. A note whose folder is deleted, purged or unknown is shown at the root, outside any folder. A folder whose parent is deleted or unknown is shown at the root. Deleting a folder in the interface writes `deleted` to the folder, its subfolders and their notes in one change (R14 in [product.md](product.md)); restoring a note from the Trash into a folder that is itself in the Trash restores the folder chain too.
- S11. The order of folders within a parent and of filters is set by the `pos` field, a string such that a third value can be built between any two. With equal `pos`, the identifier decides. Notes have no manual order: they are sorted by the setting of their folder (R16).
- S12. Settings shared by all devices are the entity `settings/app`; its fields merge by S2. Fields: `trash_days` (a whole number of days from 1 to 3650, without the field 30), `font_body`, `font_mono`, `line_height`, `paragraph_spacing`, `align` (`left` or `justify`), `hyphenate`, `smart_typography`, `spellcheck`, `spell_language` (`auto` or a BCP 47 tag) and `new_note_folder` (`root`, `last` or a folder identifier). The meaning and the defaults are in [product.md](product.md) and [editor.md](editor.md). A field with a value the version cannot parse applies its default; a version that does not know a field keeps it and passes it on. The font size and the column width are settings of the device and do not travel.

### Remote storage

```
notes/
  vault.json                    {"format":1,"vault":"<uuid>"}
  log/<device>-<number>.jsonl
  log/<device>-<number>.snap    empty file: the device has a snapshot with this number
  snap/<device>-<number>.jsonl  gzip-compressed
  blobs/<first two characters>/<sha256>
  blobs/<first two characters>/<HMAC>   content in an encrypted storage (S30)
  push/<device>.json            {"v":1,"url":"https://…"}
  backups/<device>/Notes_(…).notesbackup
```

- S13. A `log/…` file is immutable and belongs to one device; the numbers go in a row starting from one. The first line is a header with the format, the device, the number and the count of changes, then one change per line. A register change is `{"k":"<kind>","id":"<id>","f":"<field>","v":<JSON>,"t":"<timestamp>"}`. A body change is `{"k":"note","id":"<id>","f":"body","crdt":"<base64 of the Automerge changes>","t":"<timestamp>"}`; its timestamp is used only to compute the moment of the last edit, not to decide between values. A file with a wrong number of lines or an unreadable line counts as unfinished: the device does not move past it and repeats the read on the next sync. As S8 of Lists.
- S14. A device remembers, for every other device, the number of the last file it read. Sync: a request for the listing of `log/`, reading the new files, uploading its own changes as a new file. The content of the file is fixed locally before the first upload attempt, so a retry after a failure sends the same bytes under the same number. As S9 of Lists.
- S15. Snapshots and their compaction follow S10, S11, S45 and S46 of Lists: a snapshot is the full state up to a number of the device's own log plus a table "device → last file taken into account"; it is written when the log since the previous snapshot weighs more than the snapshot itself or numbers more than 256 files; it is compressed with gzip; a marker in `log/` announces it; a device that is up to date does not read snapshots. In a snapshot the body of a note is the whole saved Automerge document with its full history (S21).
- S16. A device connected to a non-empty storage for the first time reads the logs and snapshots of the others, then uploads its own state. The local data is not erased: this is a merge, and it is also the default choice in S35. When the storage changes, the numbering of the device's own log continues, and the first sync publishes a snapshot. As S12 of Lists.
- S17. A `vault.json` with an unknown format number stops sync with a clear error; local work goes on. Format 1 is a storage in plain text, format 2 is an encrypted storage (S29). A sync that has nothing to read and nothing to upload costs one request, the listing of `log/`. As S14 and S17 of Lists.
- S18. The device itself creates the folders that do not exist yet in a WebDAV storage and repeats the write once after a refusal for a missing folder, as S43 of Lists. The administrator of the storage does not create folders by hand.
- S19. A network failure does not lose data: unsynced changes stay locally and go out on the next attempt.

### Text CRDT in the log

- S20. A device remembers, for each note, the heads of the version it last uploaded. A pass that uploads changes writes one body change per note edited since then. A note created on the device gets its first body change in the same file as its first register changes, so a reader never sees a note without a body document.
- S21. The Automerge history is kept in full in the first version: neither the device nor the snapshot trims it. A prototype on the texts of the author's Standard Notes backup gave 0.93 MB of saved documents for 0.75 MB of text with synthetic corrections, and 5.9 MB of body changes in the log ([architecture.md](../architecture.md)); the size is measured again on the real import, and the activity log reports the size of the largest document. Trimming is a change of the storage format (S17).
- S22. A body change may fail to apply. When its bytes are not Automerge changes, the device keeps the change as received and retries it after the next file is read; the rest of the file is applied. When the changes depend on changes the device does not have yet, Automerge keeps them queued, also in the saved document, and applies them once the missing ones arrive; the text shows what can be applied. Either way the device writes the fact into the activity log with the note identifier and the device of the file, without the text.

### Attachments

- S23. Attachment content is uploaded into `blobs/` by the hash of its content if it is not there, and downloaded on first access or in the background. Field changes do not wait for attachments. Content whose hash does not match its name is not saved on the device. As S13 of Lists.
- S24. Sync runs in two passes: the first reads and uploads fields and bodies and ends without touching `blobs/`; the second transfers attachment content. A failure on one file does not stop the others and does not count as a sync failure; the sync status gives the number of attachments waiting for transfer. A device that uploaded content sends a push (S41). As S34 of Lists.
- S25. The device downloads the content of one attachment on request of the app, outside the queue of the second pass. Content is transferred as a stream: memory does not grow with the file size. As S35 and S52 of Lists.
- S26. Content is needed while its hash is referenced by an attachment that is not deleted and whose note is not purged. A note in the Trash keeps its content: it is restored together with its attachments. An attachment whose link was removed from the text of its note is still needed (R33 in [product.md](product.md)). The device deletes content no longer needed from its attachments folder, only at the end of a pass that read the whole storage, or on start without sync. As S47 of Lists, with the note in place of the task.
- S27. Content no longer needed is deleted from `blobs/` as well, and the device then writes the field `blob/<hash>/gone`; a device that attached the same file again while offline uploads it anew when it receives this field. As S48 of Lists.

### Storage encryption

- S28. A WebDAV storage and a folder can be encrypted with a key derived from a passphrase the user makes up. Whoever holds the storage cannot read the notes and cannot plant an edit that the devices accept. The scheme is that of Lists, S54–S59 and S62: a random data key wrapped in `vault.json` with Argon2id; files sealed with XChaCha20-Poly1305 in chunks of 64 KiB with the logical path as associated data; content named by an HMAC of its hash; turning encryption on through a snapshot under a new number and removal of the plain-text files; a device that has seen encryption refuses to write in plain text; no way to turn encryption off; a forgotten passphrase loses the data in the storage, not on the devices.
- S29. An encrypted storage is format 2: `{"format":2,"vault":"<uuid>","salt":"…","nonce":"…","key":"…"}`, base64. The associated data of the wrapping is `notes vault <storage id>`, so a `vault.json` of Lists does not open here.
- S30. Encrypted: log files, snapshots (with the Automerge bodies inside them), attachment content, push addresses and backups. Visible: the names of log files (device and number), the sizes and times of files.
- S31. The passphrase is kept where the storage password is: the macOS keychain, the Android Keystore, the variable `NOTES_SYNC_PASSPHRASE` for the web server. It does not get into the database or the storage.

### Backups in the storage

- S32. A device puts its backups into the storage as `notes/backups/<device>/Notes_(…).notesbackup`: the backup archive of R70 in [product.md](product.md) without attachment content, which already lies in `blobs/`. It is uploaded after a pass without an error when the scheduled time has passed since the previous one; the device keeps at most the number of backups set in R71 and deletes its own older ones. In an encrypted storage backups are sealed with the same key. As S60 and S61 of Lists.

### Which side to keep

- S33. When the sync settings are saved with a new or different storage and there is data on both sides, the macOS and Android apps ask: "Merge" (S16, the default), "Use the data of the storage…" (S36) or "Use the data of this device…" (S37). The device has data when it has a note not in the Trash or a folder. The storage has data when a file lies in `log/` or `snap/`. The check only reads. As S36 of Lists.
- S34. If one of the sides is empty, there is no question and a merge runs. Changing only the password does not ask. The web server, where the storage is set by environment variables, does not ask.
- S35. The choice and the storage are saved in one action, so a sync waiting its turn cannot merge the data before the replacement. Declining the choice does not save the settings.
- S36. **Take the data of the storage.** The device erases its notes, folders, filters, shared settings, Automerge documents and everything it remembered about the storage, gets a new device identifier and a new Automerge actor identifier (S4), and reads the storage as if connected for the first time. Changes not uploaded yet are lost. Attachment content on disk is not erased. The settings of the device itself stay. As S37 of Lists.
- S37. **Send the data of this device.** The device remembers the values of all its fields and the text of every body, syncs in the ordinary way, then in one write gives every remembered field its value with a new timestamp, writes into each body the edit that turns its current text into the remembered one, and marks as `deleted` every note, folder, attachment and filter it did not have. Then it syncs once more. What was only in the storage lies in the Trash on all devices. As S38 and S39 of Lists, with the body edit added.
- S38. Restoring from a backup makes the device new to the storage: a new device identifier, a new actor identifier, all fields and bodies marked for upload. If sync is on, the app asks before restoring: "Merge" or "Replace the data of the storage" (S37). As S40 of Lists.

### When it starts

- S39. After a local edit, after two seconds of quiet; on Android the start is handed to the system, so it happens even if the app is closed by then, and without a network it waits for one. When the app comes to the foreground. While the app is active, once a minute. On Android, in addition, as a background task every 15 minutes when there is a network, and by swiping down at the top of the list of notes. With sync through a folder, the macOS app and the web server also start it when a file appears in or disappears from `log/`. As S15 of Lists.
- S40. While a note is open in the editor, edits from the storage are applied to it in place: the text changes under the cursor without closing the note, and the cursor and the selection keep their place relative to the surrounding text.

### Push between devices

The storage cannot send notifications, so a change made elsewhere waits for the next run under S39. A push shortens the wait: the device that uploaded changes asks the others to sync. A push carries no data. The rules are those of Lists, S18–S31 and S41, with the names of Notes.

- S41. A device that has a receive address publishes it in `push/<device>.json` and removes the file when receiving is turned off. After a pass that uploaded changes or attachments, the device sends a `POST` with the body `sync` and the header `X-Title: <device>` to each address of the other devices; an address recorded by several devices gets one request. The requests go out at the same time, after the pass, and the pass does not wait for them. An answer is waited for no longer than five seconds; an error or silence does not affect the sync. Android waits for the sending to end before it reports a background job as done.
- S42. On receiving a push, the app starts an ordinary sync; it uploads nothing, so there is no push in reply.
- S43. The macOS app and the web server receive pushes through an [ntfy](https://ntfy.sh) server, public or one's own, whose address is set in the settings (`NOTES_PUSH_SERVER` for the web server). The device picks a random topic of 32 hexadecimal characters once, publishes `<server>/<topic>` and keeps a subscription to it while it runs. Devices of one storage on the same server converge on one topic: each takes the smallest of the addresses of the form `<its server>/<32 hexadecimal characters>` found in `push/`, including its own. A device skips a message with its own identifier in the header. A subscription that breaks after it was established counts as a push; a failed connection is retried no more often than once in ten seconds.
- S44. The Android app receives pushes through [UnifiedPush](https://unifiedpush.org): a distributor app on the device (for example, ntfy) holds the connection and gives the receive address. Without a distributor, receiving cannot be turned on, and the app says so. On a push the app schedules sync as an expedited system job, so it runs even when the app is closed. The sync settings show the row "Background work" with the state of the battery optimisation exemption and open the system request from it, as S31 of Lists.
- S45. To an ntfy server that requires sign-in the device presents `Authorization: Bearer <token>`. The token is set in the sync settings behind the switch "The push server requires sign-in" (for the web server, `NOTES_PUSH_TOKEN`), kept where the storage password is, and sent only to an address whose scheme, host and port are those of the push server in the settings; it does not follow a redirect. On Android the token is used only for sending, since the distributor receives under its own account. An answer of 401 or 403 from the device's own server is shown in the sync settings and written to the web server's log. As S27–S30 of Lists.

### Checking the settings before saving

- S46. The WebDAV settings can be checked without saving them: the app contacts the server with the address, user name and password from the form and shows whether the collection answers, whether the first sync will create it, or what the error is (401 or 403, no such address, not WebDAV, no connection). The check only reads. As S26 of Lists.

## Invariants

- Two devices that received the same set of changes in any order show the same data, including the same text of every note.
- The storage has no file that two devices write, except `notes/vault.json`, which is created once and rewritten on encryption and on a change of the passphrase; the addresses in `push/`, which the device turning on encryption rewrites encrypted; and the content in `blobs/`, where the name determines the bytes.
- The file format is versioned by a number in `vault.json` and in the header of each file.

## Known limitations

- A device whose clock has run far ahead wins conflicts over the same register until the others see its stamps. The body is not affected: Automerge does not use the clock to merge text.
- Two concurrent edits of markup at the same place of a text can leave stray markers (for example `****`); the user corrects them by hand.
- The Automerge history of a note only grows in the first version (S21). Rows of deleted records stay in the database and in snapshots, as in Lists; the body of a purged note does not (S7).
- The log and the `push/` entry of a device that will never appear again stay in the storage until they are deleted by hand.
- The push receive address lies in the storage in the open (unless it is encrypted), and devices send a `POST` to any address from there; such a request carries no data.
- In an encrypted storage the names of the log files, their sizes and times are visible.

## Test scenarios

Each requirement is covered by a test in `core/tests/` named after its identifier (`s3_*`, `s20_*` and so on). The scenarios carried over from Lists are adapted to notes and folders; the new ones:

- S3: A and B have a note "one two three"; offline, A inserts "alpha" at the start, B appends "omega"; after both sync, both have "alpha one two three omega". A and B both type at the same position → both insertions are present, in the same order on both devices. A deletes a paragraph, B edits a word inside it → both converge on the same text. The same edits delivered in reverse order give the same text.
- S4: S36 on device A → the new actor identifier differs from the old one; edits made after it merge with edits made before it on other devices.
- S7, S26: A purges a note with two attachments → on B the note disappears, its Automerge document and index entry are gone, and after the second pass the content of both attachments is gone from B and from `blobs/`; an attachment of the same file in another note keeps its content.
- S8: a note in the Trash for longer than `trash_days` → purged on the next full pass; with `trash_days` changed on another device, the new value applies after it arrives.
- S9, S10: A moves folder X into Y, B moves Y into X while offline → after sync both show the same tree without a cycle. A deletes a folder with a subfolder and notes → all go to the Trash on B; restoring one note restores its folder chain.
- S13, S20: a log file cut off in the middle of a body change → the device does not move past it and reads it after it is complete; a new note on A reaches B with its body in the same pass.
- S15, S21: a device that has never synced reads a storage of a snapshot plus a log tail and gets the same text of every note as a device that read the whole log.
- S22: a body change with damaged bytes → the rest of the file applies, the note keeps its previous text, and the activity log has a line naming the note and the device without the text. B receives A's second edit of a note before the first (its dependency) → the text shows what can be applied; after a restart of B and the arrival of the first edit, B has both.
- S28–S31: the scenarios of Lists for encryption, with a note body and an attachment of 200 000 bytes; neither file names nor contents in the storage contain the title, the text or the SHA-256 of the attachment.
- S40: B has a note open in the editor with the cursor in the middle; A inserts text before the cursor; after sync the text on B contains the insertion and the cursor is still between the same two characters.
- S71: devices A and B are named "Mac" and "Pixel"; B edits a note and syncs → the version of that edit on A names "Pixel".
- S70: an insertion at a UTF-16 position after an emoji lands after it; a position inside the emoji or past the end of the text is refused, and the text does not change. Two edits committed at 10:00:00 and 10:00:30 carry these times in milliseconds; a commit while the device clock reads earlier than the last time in the document carries that last time; the times are read on a device that received them by a body change and by a snapshot.
- S41–S45: the push scenarios of Lists (`core/tests/push.rs` there) against stub servers.
