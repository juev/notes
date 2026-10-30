package org.evsyukov.notes.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.PushPin
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.ListItem
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Text
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import org.evsyukov.notes.R
import org.evsyukov.notes.ViewDisplay
import org.evsyukov.notes.shownTitle
import uniffi.notes_core.Attachment
import uniffi.notes_core.Note
import uniffi.notes_core.Scope
import uniffi.notes_core.SortBy
import java.text.DateFormat
import java.time.Instant
import java.time.LocalDate
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Date

/** The list of notes of the open view (R5, R16, R17, R46). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun NoteList(padding: PaddingValues) {
    var menu by remember { mutableStateOf<Note?>(null) }
    var moving by remember { mutableStateOf<Note?>(null) }
    var refreshing by remember { mutableStateOf(false) }
    val display = AppModel.display
    val notes = AppModel.notes
    PullToRefreshBox(
        refreshing,
        onRefresh = {
            refreshing = true
            AppModel.syncNow { refreshing = false }
        },
        Modifier.fillMaxSize().padding(padding),
    ) {
        LazyColumn(Modifier.fillMaxSize()) {
            if (AppModel.folderOfScope?.daily == true) {
                days(notes).forEach { (day, entries) ->
                    item(key = day) {
                        Text(day, Modifier.padding(16.dp, 12.dp, 16.dp, 4.dp), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                    }
                    items(entries, key = { it.id }) { NoteRow(it, display, { menu = it }) }
                }
            } else {
                items(notes, key = { it.id }) { NoteRow(it, display, { menu = it }) }
            }
        }
        if (notes.isEmpty()) {
            Text(stringResource(R.string.no_notes), Modifier.align(Alignment.Center), color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
    menu?.let { note -> NoteMenu(note, { menu = null }, { moving = note }) }
    moving?.let { note ->
        MoveDialog({ folder ->
            moving = null
            AppModel.moveNotes(listOf(note.id), folder)
        }, { moving = null })
    }
}

/** R17: the notes of a folder in daily mode by the day they were made, the newest first. */
private fun days(notes: List<Note>): List<Pair<String, List<Note>>> {
    val form = DateTimeFormatter.ofPattern("d MMMM yyyy")
    return notes.groupBy { LocalDate.ofInstant(Instant.ofEpochMilli(it.createdMs.toLong()), ZoneId.systemDefault()) }
        .toSortedMap(compareByDescending { it })
        .map { (day, entries) -> day.format(form) to entries }
}

/** One note of the list (R5). */
@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun NoteRow(note: Note, display: ViewDisplay, onMenu: (Note) -> Unit) {
    val trash = AppModel.scope == Scope.Trash
    Column(
        Modifier.fillMaxWidth()
            .combinedClickable(onClick = { if (!trash) AppModel.open(note.id) else onMenu(note) }, onLongClick = { onMenu(note) })
            .padding(horizontal = 16.dp, vertical = 10.dp),
        verticalArrangement = Arrangement.spacedBy(2.dp),
    ) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (note.pinned) RowIcon(Icons.Filled.PushPin)
            Text(
                note.shownTitle(stringResource(R.string.untitled)),
                Modifier.weight(1f),
                fontWeight = FontWeight.Medium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            if (note.starred) RowIcon(Icons.Filled.Star, MaterialTheme.colorScheme.tertiary)
            if (note.locked) RowIcon(Icons.Filled.Lock)
            if (note.attachments > 0u) RowIcon(Icons.Outlined.AttachFile)
        }
        if (display.showPreview && note.preview.isNotEmpty()) {
            Text(
                note.preview,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 2,
                overflow = TextOverflow.Ellipsis,
            )
        }
        if (trash) {
            val left = AppModel.daysLeft(note)
            Text(pluralStringResource(R.plurals.days_left, left, left), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        } else if (display.showDate) {
            val time = if (display.sort.by == SortBy.CREATED) note.createdMs else note.editedMs
            Text(
                DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(time.toLong())),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
}

@Composable
private fun RowIcon(icon: androidx.compose.ui.graphics.vector.ImageVector, tint: Color = MaterialTheme.colorScheme.onSurfaceVariant) {
    Icon(icon, null, Modifier.padding(horizontal = 2.dp).size(14.dp), tint = tint)
}

/** What can be done with a note, from a long press in the list. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun NoteMenu(note: Note, onDismiss: () -> Unit, onMove: () -> Unit) {
    fun act(action: () -> Unit) {
        onDismiss()
        action()
    }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.padding(bottom = 16.dp)) {
            @Composable
            fun row(title: Int, action: () -> Unit) {
                MenuRow(stringResource(title)) { act(action) }
            }
            if (note.deleted) {
                row(R.string.restore) { AppModel.restore(note.id) }
                row(R.string.delete_for_good) { AppModel.purge(note.id) }
            } else {
                row(if (note.pinned) R.string.unpin else R.string.pin) { AppModel.setFlag(NoteFlag.PINNED, !note.pinned, note.id) }
                row(if (note.starred) R.string.remove_star else R.string.star) { AppModel.setFlag(NoteFlag.STARRED, !note.starred, note.id) }
                row(if (note.archived) R.string.unarchive else R.string.archive) { AppModel.setFlag(NoteFlag.ARCHIVED, !note.archived, note.id) }
                row(if (note.locked) R.string.unlock else R.string.lock) { AppModel.setFlag(NoteFlag.LOCKED, !note.locked, note.id) }
                row(R.string.move_to, onMove)
                row(R.string.duplicate) { AppModel.duplicate(note.id) }
                row(R.string.delete) { AppModel.trash(note.id) }
            }
        }
    }
}

@Composable
fun MenuRow(title: String, onClick: () -> Unit) {
    Box(Modifier.fillMaxWidth().combinedClickableRow(onClick)) { ListItem(headlineContent = { Text(title) }, colors = ListItemDefaults.colors(containerColor = Color.Transparent)) }
}

@OptIn(ExperimentalFoundationApi::class)
private fun Modifier.combinedClickableRow(onClick: () -> Unit) = combinedClickable(onClick = onClick)

/** R37: all attachments of notes that are not in the Trash, with their note. */
@Composable
fun FileList(padding: PaddingValues, onOpen: (Attachment, Note) -> Unit, onShow: (Attachment, Note) -> Unit) {
    var files by remember { mutableStateOf<List<Pair<Attachment, Note>>>(emptyList()) }
    var menu by remember { mutableStateOf<Pair<Attachment, Note>?>(null) }
    val revision = AppModel.textRevision
    LaunchedEffect(revision) { AppModel.allFiles { files = it } }
    Box(Modifier.fillMaxSize().padding(padding)) {
        LazyColumn(Modifier.fillMaxSize()) {
            items(files, key = { it.first.id }) { entry ->
                val (file, note) = entry
                FileRow(file, note, { onOpen(file, note) }, { menu = entry })
            }
        }
        if (files.isEmpty()) Text(stringResource(R.string.no_files), Modifier.align(Alignment.Center), color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    menu?.let { entry ->
        ConfirmDialog(entry.first.name, stringResource(R.string.show_in_note), { onShow(entry.first, entry.second) }, { menu = null })
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun FileRow(file: Attachment, note: Note, onClick: () -> Unit, onLong: () -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    Column(Modifier.fillMaxWidth().combinedClickable(onClick = onClick, onLongClick = onLong).padding(16.dp, 10.dp)) {
        Text(file.name, fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        Row {
            Text(file.mime, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(8.dp))
            Text(
                android.text.format.Formatter.formatShortFileSize(context, file.size.toLong()),
                style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
        Text(note.shownTitle(stringResource(R.string.untitled)), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
    }
    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
}
