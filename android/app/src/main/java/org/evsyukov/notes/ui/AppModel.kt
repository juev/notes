package org.evsyukov.notes.ui

import android.content.Context
import android.content.Intent
import android.net.Uri
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import org.evsyukov.notes.AttachmentLink
import org.evsyukov.notes.DevicePrefs
import org.evsyukov.notes.NotesApp
import org.evsyukov.notes.R
import org.evsyukov.notes.Repo
import org.evsyukov.notes.ViewDisplay
import org.evsyukov.notes.editor.AttachmentKind
import org.evsyukov.notes.editor.EditCoalescer
import org.evsyukov.notes.editor.TextEdit
import org.evsyukov.notes.editor.attachmentKind
import org.evsyukov.notes.logError
import org.evsyukov.notes.logged
import uniffi.notes_core.AppException
import uniffi.notes_core.AppSettings
import uniffi.notes_core.Attachment
import uniffi.notes_core.Counts
import uniffi.notes_core.ExportReport
import uniffi.notes_core.FilterQuery
import uniffi.notes_core.Folder
import uniffi.notes_core.ImportOptions
import uniffi.notes_core.NewNote
import uniffi.notes_core.Note
import uniffi.notes_core.NoteLinkState
import uniffi.notes_core.SavedFilter
import uniffi.notes_core.Scope
import uniffi.notes_core.SearchHit
import uniffi.notes_core.Store
import uniffi.notes_core.SyncConfig
import uniffi.notes_core.SyncStatus
import uniffi.notes_core.TagCount
import uniffi.notes_core.TextAlign
import uniffi.notes_core.Version
import uniffi.notes_core.VersionDiff
import java.io.File
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter

/** The flags of a note (R25–R28). */
enum class NoteFlag { PINNED, STARRED, ARCHIVED, LOCKED }

/**
 * The state of the app and its way to the core. It holds what the screens show; every call to the
 * core goes through [Repo.core], off the main thread, and the result comes back to the main thread.
 */
object AppModel {
    private val main = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)

    var startupError by mutableStateOf<String?>(null)
        private set

    /** What a screen shows as a message, dismissed by the user. */
    var alert by mutableStateOf<String?>(null)

    /** The view open in the sidebar (R44). */
    var scope by mutableStateOf<Scope>(Scope.All)
        private set
    var notes by mutableStateOf<List<Note>>(emptyList())
        private set
    var counts by mutableStateOf(Counts(0u, 0u, 0u, 0u, 0u, 0u))
        private set
    var folders by mutableStateOf<List<Folder>>(emptyList())
        private set
    var tags by mutableStateOf<List<TagCount>>(emptyList())
        private set
    var filters by mutableStateOf<List<SavedFilter>>(emptyList())
        private set
    var settings by mutableStateOf(
        AppSettings(30u, "system", "system", 1.5, 0.5, TextAlign.LEFT, false, true, true, "auto", "root"),
    )
        private set
    var syncStatus by mutableStateOf(SyncStatus(false, 0u, 0u, null, null, false))
        private set
    var canUndo by mutableStateOf(false)
        private set
    var canRedo by mutableStateOf(false)
        private set

    /** R37: the view "Files" is open in place of the list of notes. */
    var filesOpen by mutableStateOf(false)

    /** R16: the folder whose notes are open, or null. */
    val folderOfScope: Folder? get() = (scope as? Scope.Folder)?.let { open -> folders.firstOrNull { it.id == open.id } }

    /** R12: the folders collapsed in the sidebar, a setting of this device. */
    var foldedFolders by mutableStateOf(DevicePrefs.foldedFolders(NotesApp.instance))
        private set

    /** The note open in the editor. */
    var openNote by mutableStateOf<String?>(null)
        private set

    /** The attachments of the open note. */
    var attachments by mutableStateOf<List<Attachment>>(emptyList())
        private set

    /** Grows when the data was read again, so that an open text compares itself with the core. */
    var textRevision by mutableIntStateOf(0)
        private set

    /** R52: the note in the Trash a link leads to, while the offer to restore it is on show. */
    var restoreOffer by mutableStateOf<String?>(null)

    /** R2: a new note opens with the cursor in its title. */
    var titleFocus = false

    /** R17: the note of today opens with the cursor at the end of its text. */
    var cursorAtEnd = false

    /** R37: the text to find when the note opens: "Show in note". */
    var revealPiece: String? = null

    /** R3: a note made here and not edited yet; it is deleted without going to the Trash when left so. */
    private var freshNote: String? = null

    /** What was typed and has not reached the core yet, the text it was typed over and the whole text it gives. */
    private val edits = EditCoalescer()
    private var editedNote: String? = null
    private var editBase = ""
    private var editorText = ""
    private var editFlush: Job? = null

    private inline fun <T> message(run: () -> T): T? = try {
        run()
    } catch (error: Exception) {
        main.launch { alert = describe(error) }
        null
    }

    /** Reads again what the screens show. */
    fun reload() {
        flushEdits()
        Repo.scope.launch(Repo.core) {
            val store = runCatching { Repo.store }.getOrElse { error ->
                withContext(Dispatchers.Main) { startupError = describe(error) }
                return@launch
            }
            val folders = runCatching { store.folders() }.getOrDefault(folders)
            val tags = runCatching { store.tags() }.getOrDefault(tags)
            val filters = runCatching { store.filters() }.getOrDefault(filters)
            val counts = runCatching { store.counts() }.getOrDefault(counts)
            val settings = runCatching { store.settings() }.getOrDefault(settings)
            val status = runCatching { store.syncStatus() }.getOrDefault(syncStatus)
            val canUndo = store.canUndo()
            val canRedo = store.canRedo()
            val open = openNote
            val files = open?.let { id -> runCatching { store.attachments(id) }.getOrNull() }
            withContext(Dispatchers.Main) {
                this@AppModel.folders = folders
                this@AppModel.tags = tags
                this@AppModel.filters = filters
                this@AppModel.counts = counts
                this@AppModel.settings = settings
                syncStatus = status
                this@AppModel.canUndo = canUndo
                this@AppModel.canRedo = canRedo
                if (files != null && open == openNote) attachments = files
                // A view whose folder, tag or filter is gone falls back to all notes.
                val gone = when (val view = scope) {
                    is Scope.Folder -> folders.none { it.id == view.id }
                    is Scope.Tag -> tags.none { it.name.equals(view.name, ignoreCase = true) }
                    is Scope.Filter -> filters.none { it.id == view.id }
                    else -> false
                }
                if (gone) scope = Scope.All
                reloadNotes()
                textRevision++
            }
        }
    }

    /** The notes of the open view, in the order of its display (R16, R25). */
    private fun reloadNotes() {
        val view = scope
        val display = display
        Repo.scope.launch(Repo.core) {
            val read = runCatching { Repo.store.notes(view) }.getOrElse { return@launch }
            val shown = if (view == Scope.Trash || view is Scope.Folder) read else ViewDisplay.sorted(read, display.sort)
            withContext(Dispatchers.Main) { if (scope == view) notes = shown }
        }
    }

    fun select(view: Scope) {
        filesOpen = false
        if (view != scope) {
            scope = view
            reloadNotes()
        }
    }

    /** R16: how the open view shows its notes: a folder by its own settings, a system view by this device's. */
    val display: ViewDisplay
        get() = folderOfScope?.let { ViewDisplay(it.sort, it.showPreview, it.showDate) }
            ?: DevicePrefs.display(NotesApp.instance, ViewDisplay.key(scope))

    fun setDisplay(new: ViewDisplay) {
        val folder = folderOfScope
        if (folder != null) {
            perform { store ->
                store.setFolderSort(folder.id, new.sort)
                store.setFolderDisplay(folder.id, new.showPreview, new.showDate)
            }
        } else {
            DevicePrefs.setDisplay(NotesApp.instance, ViewDisplay.key(scope), new)
            reloadNotes()
        }
    }

    fun children(parent: String?): List<Folder> = folders.filter { it.parentId == parent }

    /** The folders from the root to this one, for its full name. */
    fun path(id: String): List<Folder> {
        val out = mutableListOf<Folder>()
        var next: String? = id
        while (next != null) {
            val folder = folders.firstOrNull { it.id == next } ?: break
            if (out.any { it.id == folder.id }) break
            out.add(0, folder)
            next = folder.parentId
        }
        return out
    }

    fun setFolded(id: String, folded: Boolean) {
        foldedFolders = if (folded) foldedFolders + id else foldedFolders - id
        DevicePrefs.setFoldedFolders(NotesApp.instance, foldedFolders)
    }

    /** R46: the days left before a note in the Trash is deleted for good. */
    fun daysLeft(note: Note): Int {
        val deleted = note.deletedMs ?: return settings.trashDays.toInt()
        val gone = (System.currentTimeMillis() - deleted.toLong()) / 86_400_000
        return maxOf(0, settings.trashDays.toInt() - gone.toInt())
    }

    /**
     * Runs a change as one action of undo (R75): the core keeps the history. A change that fails is
     * shown. [then] gets the result on the main thread.
     */
    fun <T> perform(then: (T) -> Unit = {}, change: (Store) -> T) {
        flushEdits()
        Repo.scope.launch(Repo.core) {
            val store = Repo.store
            store.beginGroup()
            val result = try {
                message { change(store) }
            } finally {
                store.endGroup()
            }
            if (result != null) withContext(Dispatchers.Main) { then(result) }
            Repo.changed()
        }
    }

    fun undo() = takeBack { it.undo() }

    fun redo() = takeBack { it.redo() }

    private fun takeBack(step: (Store) -> Unit) {
        flushEdits()
        Repo.scope.launch(Repo.core) {
            message { step(Repo.store) }
            Repo.changed()
        }
    }

    // The open note

    fun open(id: String) {
        val left = openNote
        if (left == id) return
        flushEdits()
        openNote = id
        attachments = emptyList()
        Repo.scope.launch(Repo.core) {
            val files = runCatching { Repo.store.attachments(id) }.getOrDefault(emptyList())
            withContext(Dispatchers.Main) { if (openNote == id) attachments = files }
        }
        if (left != null) dropIfUntouched(left)
    }

    fun closeNote() {
        val left = openNote ?: return
        flushEdits()
        openNote = null
        dropIfUntouched(left)
    }

    /** The note was edited: it is no longer one to drop (R3). */
    fun touched(id: String) {
        if (freshNote == id) freshNote = null
    }

    /** R3: an empty note left without any edit is deleted without going to the Trash. */
    private fun dropIfUntouched(id: String) {
        if (freshNote != id) return
        freshNote = null
        Repo.scope.launch(Repo.core) {
            val store = Repo.store
            val note = runCatching { store.getNote(id) }.getOrNull() ?: return@launch
            if (note.title.isNotEmpty() || runCatching { store.getText(id) }.getOrNull()?.isNotEmpty() != false) return@launch
            runCatching { store.purgeNote(id) }
            Repo.changed()
        }
    }

    val hasPendingEdits: Boolean get() = edits.pending != null

    /** What the editor of the open note holds, as it came to be: by its own edits and the ones it took from the core. */
    fun editorShows(text: String) {
        editorText = text
    }

    /**
     * R4: what is typed is saved as it is typed. The edits of a word reach the core together: it
     * opens the document for each edit, and a large note takes it tens of milliseconds.
     */
    fun textEdited(id: String, edit: TextEdit, whole: String) {
        touched(id)
        if (editedNote != id) flushEdits()
        editedNote = id
        if (edits.pending == null) editBase = editorText
        editorText = whole
        val earlier = edits.add(edit)
        if (earlier != null) {
            val base = editBase
            editBase = earlier.applied(base)
            Repo.scope.launch(Repo.core) { deliver(earlier, base, id) }
        }
        editFlush?.cancel()
        editFlush = main.launch {
            delay(400)
            flushEdits()
        }
    }

    /** Everything typed so far goes to the core: before another note opens, before the data is read again, and when the app leaves the screen. */
    fun flushEdits() {
        pendingDelivery()?.let { Repo.scope.launch(Repo.core) { it() } }
    }

    /** The same for the moment the app goes away: the call returns when the text is in the core. */
    fun flushEditsNow() {
        pendingDelivery()?.let { runBlocking(Repo.core) { it() } }
    }

    private fun pendingDelivery(): (() -> Unit)? {
        editFlush?.cancel()
        val id = editedNote ?: return null
        val edit = edits.take() ?: return null
        val base = editBase
        return {
            // Nothing is put right with the whole text: a sync runs beside this, and writing the
            // string would delete what it just brought. Where the two differ, the editor follows the core.
            deliver(edit, base, id)
            Repo.changed()
        }
    }

    /**
     * One edit into the core. Its positions are those of [base], the text in the editor; what a sync added
     * to the core since, seen or not, moves it to its place now, so that both stay (S3).
     */
    private fun deliver(edit: TextEdit, base: String, id: String) {
        val store = Repo.store
        val other = runCatching { TextEdit.between(base, store.getText(id)) }.getOrNull()
        val placed = other?.let(edit::rebased) ?: edit
        message { store.spliceUtf16(id, placed.position.toUInt(), placed.deleted.toUInt(), placed.inserted) }
    }

    fun setTitle(id: String, title: String) {
        touched(id)
        perform<Unit> { it.setTitle(id, title) }
    }

    fun addTag(id: String, tag: String) {
        touched(id)
        perform<Unit> { it.addTag(id, tag) }
    }

    fun removeTag(id: String, tag: String) = perform<Unit> { it.removeTag(id, tag) }

    /** E13: a tapped link: a note opens here, an address in the system. */
    fun openLink(url: String) {
        if (url.startsWith("note:")) {
            val id = url.removePrefix("note:")
            linkState(id) { state ->
                when (state) {
                    NoteLinkState.LIVE -> open(id)
                    // R52: a tap on a link to a note in the Trash offers to restore it.
                    NoteLinkState.IN_TRASH -> restoreOffer = id
                    NoteLinkState.GONE -> {}
                }
            }
        } else {
            val context = NotesApp.instance
            val intent = Intent(Intent.ACTION_VIEW, Uri.parse(url)).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            runCatching { context.startActivity(intent) }.onFailure { alert = describe(it) }
        }
    }

    /** R53: the selection moves into a new note, and a link stays in its place. */
    fun noteFromSelection(id: String, start: Int, end: Int) = perform<Unit> { it.noteFromSelection(id, start.toUInt(), end.toUInt()) }

    // Notes

    /** R2: a new note in the open folder, or where `new_note_folder` says; R17: in a folder of daily mode, today's note is opened instead when there is one. */
    fun newNote() {
        val open = folderOfScope
        Repo.scope.launch(Repo.core) {
            val store = Repo.store
            val folderId = open?.id ?: runCatching { store.defaultFolderId(DevicePrefs.lastFolder(NotesApp.instance)) }.getOrNull()
            val folder = folderId?.let { id -> runCatching { store.folder(id) }.getOrNull() }
            val daily = folder?.daily == true
            val today = if (daily) todaysNote(store, folder.id) else null
            withContext(Dispatchers.Main) {
                if (today != null) {
                    cursorAtEnd = true
                    open(today.id)
                } else {
                    createNote(folderId, daily)
                }
            }
        }
    }

    /** R17: "New entry for today" makes a second note of the day. */
    fun newEntryForToday() {
        val folder = folderOfScope ?: return newNote()
        createNote(folder.id, folder.daily)
    }

    private fun createNote(folderId: String?, daily: Boolean) {
        val title = if (daily) LocalDate.now().format(DateTimeFormatter.ofPattern("d MMMM yyyy")) else ""
        perform({ note: Note ->
            if (folderId != null) DevicePrefs.setLastFolder(NotesApp.instance, folderId)
            titleFocus = !daily
            cursorAtEnd = daily
            // After the note before it is left: that one may have to go (R3).
            open(note.id)
            freshNote = if (daily) null else note.id
        }) { it.createNote(NewNote(title, folderId, emptyList(), "")) }
    }

    /** R17: the note of the folder made today, if there is one. */
    private fun todaysNote(store: Store, folderId: String): Note? {
        val today = LocalDate.now()
        return runCatching { store.notes(Scope.Folder(folderId)) }.getOrDefault(emptyList())
            .filter { Instant.ofEpochMilli(it.createdMs.toLong()).atZone(ZoneId.systemDefault()).toLocalDate() == today }
            .maxByOrNull { it.createdMs }
    }

    fun duplicate(id: String) = perform({ copy: Note -> open(copy.id) }) { it.duplicateNote(id) }

    fun moveNotes(ids: List<String>, folderId: String?) = perform<Unit> { store -> ids.forEach { store.moveNote(it, folderId) } }

    fun setFlag(flag: NoteFlag, on: Boolean, id: String) = perform<Unit> { store ->
        when (flag) {
            NoteFlag.PINNED -> store.setPinned(id, on)
            NoteFlag.STARRED -> store.setStarred(id, on)
            NoteFlag.ARCHIVED -> store.setArchived(id, on)
            NoteFlag.LOCKED -> store.setLocked(id, on)
        }
    }

    fun trash(id: String) {
        if (openNote == id) closeNote()
        perform<Unit> { it.deleteNote(id) }
    }

    fun restore(id: String, then: () -> Unit = {}) = perform({ _: Unit -> then() }) { it.restoreNote(id) }

    /** R46: for good, with no undo: the core keeps nothing to bring back. */
    fun purge(id: String) {
        if (openNote == id) closeNote()
        Repo.scope.launch(Repo.core) {
            message { Repo.store.purgeNote(id) }
            Repo.changed()
        }
    }

    fun emptyTrash() {
        closeNote()
        Repo.scope.launch(Repo.core) {
            message { Repo.store.emptyTrash() }
            Repo.changed()
        }
    }

    // Folders

    fun createFolder(name: String, parent: String?) = perform({ folder: Folder ->
        if (parent != null) setFolded(parent, false)
        select(Scope.Folder(folder.id))
    }) { it.createFolder(name, parent) }

    fun renameFolder(id: String, name: String) = perform<Unit> { it.renameFolder(id, name) }

    fun setFolderIcon(id: String, icon: String) = perform<Unit> { it.setFolderIcon(id, icon) }

    fun setDaily(id: String, on: Boolean) = perform<Unit> { it.setFolderDaily(id, on) }

    /** R11: a folder into another, or to the root; never into itself or below itself. */
    fun moveFolder(id: String, parent: String?) {
        if (parent != null && path(parent).any { it.id == id }) return
        perform<Unit> { it.moveFolder(id, parent, null) }
    }

    /** R14: how many notes a deletion takes to the Trash, for its confirmation. */
    fun deletionCount(id: String, then: (Int) -> Unit) {
        Repo.scope.launch(Repo.core) {
            val count = runCatching { Repo.store.folderDeletionCount(id).toInt() }.getOrDefault(0)
            withContext(Dispatchers.Main) { then(count) }
        }
    }

    fun deleteFolder(id: String) = perform<Unit> { it.deleteFolder(id) }

    // Tags and filters

    fun renameTag(old: String, new: String) = perform({ _: UInt ->
        (scope as? Scope.Tag)?.let { if (it.name.equals(old, ignoreCase = true)) select(Scope.Tag(new)) }
    }) { it.renameTag(old, new) }

    fun deleteTag(name: String) = perform<Unit> { it.deleteTag(name) }

    fun saveFilter(id: String?, name: String, icon: String, query: FilterQuery) {
        if (id != null) {
            perform<Unit> { it.updateFilter(id, name, icon, query) }
        } else {
            perform({ made: SavedFilter -> select(Scope.Filter(made.id)) }) { it.createFilter(name, icon, query) }
        }
    }

    fun deleteFilter(id: String) = perform<Unit> { it.deleteFilter(id) }

    /** R37: every attachment of the notes outside the Trash, the newest first. */
    fun allFiles(then: (List<Pair<Attachment, Note>>) -> Unit) {
        Repo.scope.launch(Repo.core) {
            val store = Repo.store
            val notes = runCatching { store.notes(Scope.All) + store.notes(Scope.Archived) }.getOrDefault(emptyList())
            val files = notes.filter { it.attachments > 0u }
                .sortedByDescending { it.editedMs }
                .flatMap { note -> runCatching { store.attachments(note.id) }.getOrDefault(emptyList()).map { it to note } }
            withContext(Dispatchers.Main) { then(files) }
        }
    }

    // Attachments

    /** The attachment the viewer shows, while it is on show (R31). */
    var viewing by mutableStateOf<Attachment?>(null)

    /** R30: the files go into the note; [link] puts each link at the cursor. */
    fun attach(context: Context, uris: List<Uri>, id: String, link: (String) -> Unit) {
        touched(id)
        flushEdits()
        Repo.scope.launch(Repo.core) {
            for (uri in uris) {
                val file = AttachmentFiles.copyToCache(context, uri) ?: continue
                val added = message { Repo.store.addAttachment(id, file.absolutePath, file.name) }
                file.parentFile?.deleteRecursively()
                if (added != null) withContext(Dispatchers.Main) { link(AttachmentLink.text(added)) }
            }
            Repo.changed()
        }
    }

    /** R37: "Show in note" opens the note at the place of the link. */
    fun showInNote(note: String, file: String) {
        filesOpen = false
        revealPiece = "attachment:$file"
        open(note)
    }

    /** R33: deleted by hand from the panel. */
    fun removeAttachment(id: String) = perform<Unit> { it.removeAttachment(id) }

    /** R31, R32: a tap shows the file; R35: one that has not arrived is fetched ahead of the queue. */
    fun openAttachment(context: Context, id: String, repaint: (String) -> Unit) {
        val file = attachments.firstOrNull { it.id == id } ?: return
        when (attachmentKind(file)) {
            AttachmentKind.PENDING -> fetch(id)
            AttachmentKind.IMAGE, AttachmentKind.PDF, AttachmentKind.VIDEO -> viewing = file
            AttachmentKind.AUDIO -> {
                val stopped = AudioPlayer.playing
                AudioPlayer.toggle(file) { repaint(id) }
                stopped?.let(repaint)
                repaint(id)
            }
            AttachmentKind.CHIP -> AttachmentFiles.open(context, file)
            AttachmentKind.BROKEN -> {}
        }
    }

    private fun fetch(id: String) {
        Repo.scope.launch {
            message { Repo.store.fetchAttachment(id) }
            Repo.revision.update { it + 1 }
        }
    }

    // Search and links

    fun search(query: String, archived: Boolean, trash: Boolean, then: (List<SearchHit>) -> Unit) = read(emptyList(), then) { it.search(query, archived, trash) }

    /** R50: the notes for the part of a title typed after `[[`. */
    fun titlesMatching(query: String, except: String, then: (List<Note>) -> Unit) = read(emptyList(), then) { store ->
        store.notesMatchingTitle(query, 8u).filter { it.id != except }
    }

    fun linkedFrom(id: String, then: (List<Note>) -> Unit) = read(emptyList(), then) { it.linkedFrom(id) }

    /** Reads from the core off the main thread; [fallback] when it fails. */
    private fun <T> read(fallback: T, then: (T) -> Unit, get: (Store) -> T) {
        Repo.scope.launch(Repo.core) {
            val value = runCatching { get(Repo.store) }.getOrDefault(fallback)
            withContext(Dispatchers.Main) { then(value) }
        }
    }

    // Version history (R40–R43)

    fun versions(id: String, then: (List<Version>) -> Unit) {
        flushEdits()
        read(emptyList(), then) { it.versions(id) }
    }

    fun versionDiff(id: String, version: String, then: (VersionDiff?) -> Unit) = read(null, then) { it.versionDiff(id, version) }

    /** R42: an ordinary edit, one action of undo; the later versions stay. */
    fun restoreVersion(id: String, version: String) = perform<Unit> { it.restoreVersion(id, version) }

    // Import and export

    /** An import or an export is running. */
    var working by mutableStateOf(false)
        private set

    /** I12: the report of the last import, while it is on show. */
    var importReport by mutableStateOf<String?>(null)

    /** R60–R62: the core writes into the cache, and the result goes to the folder the user picked. */
    fun export(context: Context, job: ExportJob, tree: Uri) {
        flushEdits()
        working = true
        Repo.scope.launch(Repo.core) {
            val dir = File(context.cacheDir, "export/${System.nanoTime()}").apply { mkdirs() }
            val store = Repo.store
            val message = runCatching {
                val done = when (job) {
                    is ExportJob.Markdown -> summary(context, store.exportMarkdown(job.scope, dir.absolutePath))
                    is ExportJob.Json -> summary(context, store.exportJson(dir.absolutePath, job.attachments))
                    is ExportJob.Html -> context.getString(R.string.saved_as, File(store.exportHtml(job.note, dir.absolutePath, job.embed)).name)
                }
                FolderCopy.toTree(context, dir, tree)
                done
            }.getOrElse(::describe)
            dir.deleteRecursively()
            withContext(Dispatchers.Main) {
                working = false
                alert = message
            }
        }
    }

    private fun summary(context: Context, report: ExportReport): String {
        val done = context.getString(R.string.exported, report.notes.toInt(), report.attachments.toInt())
        return if (report.missing.isEmpty()) done else done + "\n" + context.getString(R.string.not_on_device, report.missing.joinToString(", "))
    }

    /** R61: the note as a PDF, printed by the system from the HTML of the core. */
    fun printPdf(context: Context, id: String, title: String) {
        flushEdits()
        Repo.scope.launch(Repo.core) {
            val dir = File(context.cacheDir, "export/${System.nanoTime()}").apply { mkdirs() }
            val html = message { File(Repo.store.exportHtml(id, dir.absolutePath, true)) } ?: return@launch
            withContext(Dispatchers.Main) { printNote(context, html, title) }
        }
    }

    /** I2: one action of undo. */
    fun importBackup(context: Context, backup: Uri, files: Uri?, options: ImportOptions) {
        flushEdits()
        working = true
        Repo.scope.launch {
            val dir = File(context.cacheDir, "import/${System.nanoTime()}").apply { mkdirs() }
            val result = runCatching {
                val copy = AttachmentFiles.copyToCache(context, backup) ?: error("the backup was not read")
                val folder = files?.let { tree -> File(dir, "files").also { FolderCopy.fromTree(context, tree, it) } }
                val report = Repo.store.importStandardNotes(copy.absolutePath, folder?.absolutePath, options)
                copy.parentFile?.deleteRecursively()
                report.text
            }
            dir.deleteRecursively()
            withContext(Dispatchers.Main) {
                working = false
                result.onSuccess { importReport = it }.onFailure { alert = describe(it) }
            }
            Repo.changed()
        }
    }

    fun linkState(id: String, then: (NoteLinkState) -> Unit) {
        Repo.scope.launch(Repo.core) {
            val state = runCatching { Repo.store.linkTargets(listOf(id)).firstOrNull()?.state }.getOrNull() ?: NoteLinkState.GONE
            withContext(Dispatchers.Main) { then(state) }
        }
    }

    // Sync

    /** R81: whether a storage is set, for pulling the list down. */
    val syncConfigured: Boolean get() = syncStatus.configured

    /** S39: by swiping down at the top of the list of notes. */
    fun syncNow(done: () -> Unit = {}) {
        flushEdits()
        main.launch {
            Repo.sync().onFailure { logError("sync", "the sync failed: ${logged(it)}") }
            done()
        }
    }

    fun syncConfig(then: (SyncConfig) -> Unit) {
        Repo.scope.launch(Repo.core) {
            val config = runCatching { Repo.store.syncConfig() }.getOrDefault(SyncConfig.Off)
            withContext(Dispatchers.Main) { then(config) }
        }
    }
}

/** Text fit for showing to the user. */
fun describe(error: Throwable): String {
    val context = NotesApp.instance
    return when (error) {
        is AppException.NotFound -> context.getString(R.string.error_not_found)
        is AppException.Invalid -> error.msg
        is AppException.Storage -> context.getString(R.string.error_storage, error.msg)
        is AppException.Sync -> error.msg
        is AppException.TooLarge -> error.msg
        else -> error.localizedMessage ?: error.javaClass.simpleName
    }
}
