package org.evsyukov.notes.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Label
import androidx.compose.material.icons.filled.ArrowDropDown
import androidx.compose.material.icons.filled.ArrowRight
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.AttachFile
import androidx.compose.material.icons.outlined.CalendarMonth
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.FilterList
import androidx.compose.material.icons.outlined.Folder
import androidx.compose.material.icons.outlined.Inbox
import androidx.compose.material.icons.outlined.LabelOff
import androidx.compose.material.icons.outlined.MoveToInbox
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material.icons.outlined.StarOutline
import androidx.compose.material.icons.outlined.Sync
import androidx.compose.material.icons.outlined.SyncProblem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import org.evsyukov.notes.R
import org.evsyukov.notes.Repo
import uniffi.notes_core.ExportScope
import uniffi.notes_core.Folder
import uniffi.notes_core.SavedFilter
import uniffi.notes_core.Scope

/** What a long press on a row of the sidebar asks for. */
private sealed class SidebarDialog {
    data class NewFolder(val parent: String?) : SidebarDialog()
    data class RenameFolder(val folder: Folder) : SidebarDialog()
    data class FolderMenu(val folder: Folder) : SidebarDialog()
    data class DeleteFolder(val folder: Folder, val count: Int) : SidebarDialog()
    data class MoveFolder(val folder: Folder) : SidebarDialog()
    data class TagMenu(val name: String) : SidebarDialog()
    data class RenameTag(val name: String) : SidebarDialog()
    data class FilterMenu(val filter: SavedFilter) : SidebarDialog()
}

/** The sidebar (R44): the system views, the saved filters, the folders and the tags. */
@Composable
fun Sidebar(onChosen: () -> Unit, onSettings: () -> Unit) {
    var dialog by remember { mutableStateOf<SidebarDialog?>(null) }
    var filter by remember { mutableStateOf<FilterDraft?>(null) }
    val export = rememberExport()
    fun choose(scope: Scope) {
        AppModel.select(scope)
        onChosen()
    }
    val counts = AppModel.counts
    val view = AppModel.scope.takeUnless { AppModel.filesOpen }
    Column(Modifier.fillMaxWidth().verticalScroll(rememberScrollState()).padding(vertical = 12.dp)) {
        ViewRow(stringResource(R.string.all_notes), Icons.Outlined.Inbox, counts.all, view == Scope.All) { choose(Scope.All) }
        ViewRow(stringResource(R.string.files), Icons.Outlined.AttachFile, 0u, AppModel.filesOpen) {
            AppModel.filesOpen = true
            onChosen()
        }
        ViewRow(stringResource(R.string.no_folder), Icons.Outlined.MoveToInbox, counts.noFolder, view == Scope.NoFolder) { choose(Scope.NoFolder) }
        ViewRow(stringResource(R.string.starred), Icons.Outlined.StarOutline, counts.starred, view == Scope.Starred) { choose(Scope.Starred) }
        ViewRow(stringResource(R.string.archived), Icons.Outlined.Archive, counts.archived, view == Scope.Archived) { choose(Scope.Archived) }
        ViewRow(stringResource(R.string.trash), Icons.Outlined.Delete, counts.trash, view == Scope.Trash) { choose(Scope.Trash) }
        ViewRow(stringResource(R.string.untagged), Icons.Outlined.LabelOff, counts.untagged, view == Scope.Untagged) { choose(Scope.Untagged) }

        Heading(stringResource(R.string.filters)) { filter = FilterDraft() }
        AppModel.filters.forEach { saved ->
            ViewRow(saved.name, Icons.Outlined.FilterList, saved.count, view == Scope.Filter(saved.id), saved.icon, { dialog = SidebarDialog.FilterMenu(saved) }) {
                choose(Scope.Filter(saved.id))
            }
        }

        Heading(stringResource(R.string.folders)) { dialog = SidebarDialog.NewFolder(null) }
        AppModel.children(null).forEach { FolderRow(it, 0, view, ::choose) { folder -> dialog = SidebarDialog.FolderMenu(folder) } }

        Heading(stringResource(R.string.tags))
        AppModel.tags.forEach { tag ->
            ViewRow(tag.name, Icons.AutoMirrored.Outlined.Label, tag.count, view == Scope.Tag(tag.name), "", { dialog = SidebarDialog.TagMenu(tag.name) }) {
                choose(Scope.Tag(tag.name))
            }
        }

        HorizontalDivider(Modifier.padding(vertical = 8.dp))
        SyncLine()
        ViewRow(stringResource(R.string.settings), Icons.Outlined.Settings, 0u, false, onClick = onSettings)
    }

    when (val shown = dialog) {
        null -> {}
        is SidebarDialog.NewFolder -> TextDialog(stringResource(R.string.new_folder), stringResource(R.string.name), "", { name, _ -> AppModel.createFolder(name, shown.parent) }, { dialog = null })
        is SidebarDialog.RenameFolder -> TextDialog(
            stringResource(R.string.rename), stringResource(R.string.name), shown.folder.name,
            { name, icon ->
                if (name != shown.folder.name) AppModel.renameFolder(shown.folder.id, name)
                if (icon != shown.folder.icon) AppModel.setFolderIcon(shown.folder.id, icon)
            },
            { dialog = null }, stringResource(R.string.icon_emoji), shown.folder.icon,
        )
        is SidebarDialog.FolderMenu -> MenuDialog(shown.folder.name, { dialog = null }) {
            MenuRow(stringResource(R.string.new_subfolder)) { dialog = SidebarDialog.NewFolder(shown.folder.id) }
            MenuRow(stringResource(R.string.rename)) { dialog = SidebarDialog.RenameFolder(shown.folder) }
            MenuRow(stringResource(R.string.move_to)) { dialog = SidebarDialog.MoveFolder(shown.folder) }
            MenuRow(stringResource(R.string.export)) {
                dialog = null
                export(ExportJob.Markdown(ExportScope.Folder(shown.folder.id)))
            }
            MenuRow(stringResource(if (shown.folder.daily) R.string.daily_off else R.string.daily_on)) {
                dialog = null
                AppModel.setDaily(shown.folder.id, !shown.folder.daily)
            }
            MenuRow(stringResource(R.string.delete)) {
                dialog = null
                AppModel.deletionCount(shown.folder.id) { dialog = SidebarDialog.DeleteFolder(shown.folder, it) }
            }
        }
        is SidebarDialog.DeleteFolder -> ConfirmDialog(
            stringResource(R.string.delete_folder_question, shown.folder.name, shown.count),
            stringResource(R.string.move_to_trash), { AppModel.deleteFolder(shown.folder.id) }, { dialog = null },
        )
        is SidebarDialog.MoveFolder -> MoveDialog({ parent ->
            dialog = null
            AppModel.moveFolder(shown.folder.id, parent)
        }, { dialog = null }, { id -> AppModel.path(id).any { it.id == shown.folder.id } })
        is SidebarDialog.TagMenu -> MenuDialog(shown.name, { dialog = null }) {
            MenuRow(stringResource(R.string.rename)) { dialog = SidebarDialog.RenameTag(shown.name) }
            MenuRow(stringResource(R.string.delete)) {
                dialog = null
                AppModel.deleteTag(shown.name)
            }
        }
        is SidebarDialog.RenameTag -> TextDialog(stringResource(R.string.rename), stringResource(R.string.name), shown.name, { name, _ -> AppModel.renameTag(shown.name, name) }, { dialog = null })
        is SidebarDialog.FilterMenu -> MenuDialog(shown.filter.name, { dialog = null }) {
            MenuRow(stringResource(R.string.edit)) {
                dialog = null
                filter = FilterDraft(shown.filter)
            }
            MenuRow(stringResource(R.string.delete)) {
                dialog = null
                AppModel.deleteFilter(shown.filter.id)
            }
        }
    }
    filter?.let { draft -> FilterEditor(draft) { filter = null } }
}

@Composable
private fun Heading(title: String, onAdd: (() -> Unit)? = null) {
    Row(Modifier.fillMaxWidth().padding(start = 28.dp, end = 12.dp, top = 8.dp).heightIn(min = 40.dp), verticalAlignment = Alignment.CenterVertically) {
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (onAdd != null) IconButton(onAdd) { Icon(Icons.Outlined.Add, stringResource(R.string.add)) }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun ViewRow(title: String, icon: ImageVector, count: UInt, selected: Boolean, emoji: String = "", onLong: (() -> Unit)? = null, modifier: Modifier = Modifier, onClick: () -> Unit) {
    Row(
        modifier.padding(horizontal = 12.dp).fillMaxWidth().clip(CircleShape)
            .background(if (selected) MaterialTheme.colorScheme.secondaryContainer else Color.Transparent)
            .combinedClickable(onClick = onClick, onLongClick = onLong)
            .heightIn(min = 48.dp).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(12.dp),
    ) {
        if (emoji.isNotEmpty() && !emoji.all { it.code < 128 }) Text(emoji) else Icon(icon, null, tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Text(title, Modifier.weight(1f), maxLines = 1, overflow = TextOverflow.Ellipsis)
        if (count > 0u) Text(count.toString(), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

/** A folder of the tree with its subfolders (R10, R12). */
@Composable
private fun FolderRow(folder: Folder, depth: Int, view: Scope?, choose: (Scope) -> Unit, onMenu: (Folder) -> Unit) {
    val children = AppModel.children(folder.id)
    val folded = folder.id in AppModel.foldedFolders
    Row(Modifier.padding(start = (depth * 16).dp), verticalAlignment = Alignment.CenterVertically) {
        ViewRow(
            folder.name, if (folder.daily) Icons.Outlined.CalendarMonth else Icons.Outlined.Folder, folder.noteCount,
            view == Scope.Folder(folder.id), folder.icon, { onMenu(folder) }, Modifier.weight(1f),
        ) { choose(Scope.Folder(folder.id)) }
        if (children.isNotEmpty()) {
            IconButton({ AppModel.setFolded(folder.id, !folded) }, Modifier.size(40.dp)) {
                Icon(if (folded) Icons.Filled.ArrowRight else Icons.Filled.ArrowDropDown, stringResource(if (folded) R.string.unfold else R.string.fold))
            }
        }
    }
    if (!folded) children.forEach { FolderRow(it, depth + 1, view, choose, onMenu) }
}

/** R81: the state of sync; a tap starts one. */
@Composable
private fun SyncLine() {
    val syncing by Repo.syncing.collectAsState()
    val status = AppModel.syncStatus
    val failed = status.lastError
    val line = when {
        syncing -> stringResource(R.string.syncing)
        failed != null -> stringResource(R.string.sync_failed, failed)
        status.attachmentsWaiting > 0u -> stringResource(R.string.attachments_waiting, status.attachmentsWaiting.toString())
        status.configured -> stringResource(R.string.synced)
        else -> return
    }
    ViewRow(line, if (failed == null) Icons.Outlined.Sync else Icons.Outlined.SyncProblem, 0u, false) { AppModel.syncNow() }
}
