package org.evsyukov.notes.ui

import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.withContext
import org.evsyukov.notes.DevicePrefs
import org.evsyukov.notes.NotesApp
import org.evsyukov.notes.R
import org.evsyukov.notes.Repo
import org.evsyukov.notes.ViewDisplay
import org.evsyukov.notes.editor.EditCoalescer
import org.evsyukov.notes.editor.TextEdit
import org.evsyukov.notes.logged
import org.evsyukov.notes.logError
import uniffi.notes_core.AppException
import uniffi.notes_core.AppSettings
import uniffi.notes_core.Attachment
import uniffi.notes_core.Counts
import uniffi.notes_core.FilterQuery
import uniffi.notes_core.Folder
import uniffi.notes_core.NewNote
import uniffi.notes_core.Note
import uniffi.notes_core.NoteLinkState
import uniffi.notes_core.SavedFilter
import uniffi.notes_core.Scope
import uniffi.notes_core.Store
import uniffi.notes_core.SyncConfig
import uniffi.notes_core.SyncStatus
import uniffi.notes_core.TagCount
import uniffi.notes_core.TextAlign
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

    /** R3: a note made here and not edited yet; it is deleted without going to the Trash when left so. */
    private var freshNote: String? = null

    /** What was typed and has not reached the core yet, the text it was typed over and the whole text it gives. */
    private val edits = EditCoalescer()
    private var editedNote: String? = null
    private var editedText = ""
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
        editedText = whole
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
        val whole = editedText
        return {
            val merged = deliver(edit, base, id)
            // The core must hold what the editor shows; whatever got past the edits is put right. Not
            // when a sync changed the text meanwhile: then the core holds both, and the editor follows.
            if (!merged && runCatching { Repo.store.getText(id) }.getOrNull() != whole) {
                message { Repo.store.setText(id, whole) }
            }
            Repo.changed()
        }
    }

    /**
     * One edit into the core. Its positions are those of [base], the text in the editor; what a sync added
     * to the core since, seen or not, moves it to its place now, so that both stay (S3). Says whether that happened.
     */
    private fun deliver(edit: TextEdit, base: String, id: String): Boolean {
        val store = Repo.store
        val other = runCatching { TextEdit.between(base, store.getText(id)) }.getOrNull()
        val placed = other?.let(edit::rebased) ?: edit
        message { store.spliceUtf16(id, placed.position.toUInt(), placed.deleted.toUInt(), placed.inserted) }
        return other != null
    }

    fun setTitle(id: String, title: String) {
        touched(id)
        perform<Unit> { it.setTitle(id, title) }
    }

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
