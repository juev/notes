package org.evsyukov.notes.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.Redo
import androidx.compose.material.icons.automirrored.outlined.Sort
import androidx.compose.material.icons.automirrored.outlined.Undo
import androidx.compose.material.icons.filled.Menu
import androidx.compose.material.icons.outlined.Add
import androidx.compose.material.icons.outlined.DeleteSweep
import androidx.compose.material.icons.outlined.EditCalendar
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.DrawerValue
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.ModalDrawerSheet
import androidx.compose.material3.ModalNavigationDrawer
import androidx.compose.material3.RadioButton
import androidx.compose.material3.Scaffold
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.rememberDrawerState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.launch
import org.evsyukov.notes.R
import org.evsyukov.notes.Repo
import org.evsyukov.notes.ViewDisplay
import uniffi.notes_core.FolderSort
import uniffi.notes_core.Scope
import uniffi.notes_core.SortBy

/** The one screen of the app: a panel with the sidebar, the list of notes, the editor full-screen (docs/specs/product.md, "Interface"). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun MainScreen() {
    LaunchedEffect(Unit) { Repo.revision.collectLatest { AppModel.reload() } }
    var settingsOpen by remember { mutableStateOf(false) }
    val startup = AppModel.startupError
    when {
        startup != null -> Text(startup)
        AppModel.openNote != null -> {
            val id = AppModel.openNote ?: return
            BackHandler { AppModel.closeNote() }
            NoteScreen(id)
        }
        else -> Panel { settingsOpen = true }
    }
    AppModel.alert?.let { message ->
        AlertDialog(
            onDismissRequest = { AppModel.alert = null },
            text = { Text(message) },
            confirmButton = { TextButton({ AppModel.alert = null }) { Text(stringResource(R.string.ok)) } },
        )
    }
    AppModel.restoreOffer?.let { id ->
        ConfirmDialog(stringResource(R.string.note_in_trash), stringResource(R.string.restore), { AppModel.restore(id) { AppModel.open(id) } }, { AppModel.restoreOffer = null })
    }
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Panel(onSettings: () -> Unit) {
    val drawer = rememberDrawerState(DrawerValue.Closed)
    val scope = rememberCoroutineScope()
    var emptying by remember { mutableStateOf(false) }
    BackHandler(drawer.isOpen) { scope.launch { drawer.close() } }
    ModalNavigationDrawer(
        drawerState = drawer,
        drawerContent = {
            ModalDrawerSheet { Sidebar({ scope.launch { drawer.close() } }, onSettings) }
        },
    ) {
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text(title(), maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    navigationIcon = { IconButton({ scope.launch { drawer.open() } }) { Icon(Icons.Filled.Menu, stringResource(R.string.open_sidebar)) } },
                    actions = {
                        if (AppModel.canUndo) IconButton({ AppModel.undo() }) { Icon(Icons.AutoMirrored.Outlined.Undo, stringResource(R.string.undo)) }
                        if (AppModel.canRedo) IconButton({ AppModel.redo() }) { Icon(Icons.AutoMirrored.Outlined.Redo, stringResource(R.string.redo)) }
                        if (!AppModel.filesOpen) {
                            if (AppModel.scope == Scope.Trash && AppModel.notes.isNotEmpty()) {
                                IconButton({ emptying = true }) { Icon(Icons.Outlined.DeleteSweep, stringResource(R.string.empty_trash)) }
                            }
                            if (AppModel.folderOfScope?.daily == true) {
                                IconButton({ AppModel.newEntryForToday() }) { Icon(Icons.Outlined.EditCalendar, stringResource(R.string.new_entry_today)) }
                            }
                            DisplayMenu()
                        }
                    },
                )
            },
            floatingActionButton = {
                if (!AppModel.filesOpen && AppModel.scope != Scope.Trash) {
                    FloatingActionButton({ AppModel.newNote() }) { Icon(Icons.Outlined.Add, stringResource(R.string.new_note)) }
                }
            },
        ) { padding ->
            if (AppModel.filesOpen) {
                FileList(padding, { _, _ -> }, { _, note -> AppModel.open(note.id) })
            } else {
                NoteList(padding)
            }
        }
    }
    if (emptying) {
        ConfirmDialog(stringResource(R.string.empty_trash_question), stringResource(R.string.empty_trash), { AppModel.emptyTrash() }, { emptying = false })
    }
}

/** The title of the open view. */
@Composable
private fun title(): String {
    if (AppModel.filesOpen) return stringResource(R.string.files)
    return when (val scope = AppModel.scope) {
        Scope.All -> stringResource(R.string.all_notes)
        Scope.Starred -> stringResource(R.string.starred)
        Scope.Archived -> stringResource(R.string.archived)
        Scope.Trash -> stringResource(R.string.trash)
        Scope.Untagged -> stringResource(R.string.untagged)
        Scope.NoFolder -> stringResource(R.string.no_folder)
        is Scope.Folder -> AppModel.folderOfScope?.name ?: stringResource(R.string.app_name)
        is Scope.Tag -> "#${scope.name}"
        is Scope.Filter -> AppModel.filters.firstOrNull { it.id == scope.id }?.name ?: stringResource(R.string.app_name)
    }
}

/** R16: the sort and what the rows show, for the open view. */
@Composable
private fun DisplayMenu() {
    var open by remember { mutableStateOf(false) }
    val display = AppModel.display
    fun set(new: ViewDisplay) = AppModel.setDisplay(new)
    IconButton({ open = true }) { Icon(Icons.AutoMirrored.Outlined.Sort, stringResource(R.string.display)) }
    DropdownMenu(open, { open = false }) {
        Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState()).fillMaxWidth()) {
            listOf(R.string.sort_edited to SortBy.EDITED, R.string.sort_created to SortBy.CREATED, R.string.sort_title to SortBy.TITLE).forEach { (name, by) ->
                DropdownMenuItem({ Text(stringResource(name)) }, { set(display.copy(sort = FolderSort(by, display.sort.descending))) }, trailingIcon = {
                    RadioButton(display.sort.by == by, null)
                })
            }
            listOf(R.string.descending to true, R.string.ascending to false).forEach { (name, descending) ->
                DropdownMenuItem({ Text(stringResource(name)) }, { set(display.copy(sort = FolderSort(display.sort.by, descending))) }, trailingIcon = {
                    RadioButton(display.sort.descending == descending, null)
                })
            }
            DropdownMenuItem({ Text(stringResource(R.string.show_preview)) }, { set(display.copy(showPreview = !display.showPreview)) }, trailingIcon = {
                Checkbox(display.showPreview, null)
            })
            DropdownMenuItem({ Text(stringResource(R.string.show_date)) }, { set(display.copy(showDate = !display.showDate)) }, trailingIcon = {
                Checkbox(display.showDate, null)
            })
        }
    }
}

/** A short list of actions over the screen. */
@Composable
fun MenuDialog(title: String, onDismiss: () -> Unit, content: @Composable () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis) },
        text = { Column { content() } },
        confirmButton = {},
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}
