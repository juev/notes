package org.evsyukov.notes.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.FormatIndentDecrease
import androidx.compose.material.icons.automirrored.filled.FormatIndentIncrease
import androidx.compose.material.icons.automirrored.filled.FormatListBulleted
import androidx.compose.material.icons.automirrored.filled.Redo
import androidx.compose.material.icons.automirrored.filled.Undo
import androidx.compose.material.icons.filled.ArrowDownward
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.ChecklistRtl
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Code
import androidx.compose.material.icons.filled.FormatBold
import androidx.compose.material.icons.filled.FormatColorFill
import androidx.compose.material.icons.filled.FormatItalic
import androidx.compose.material.icons.filled.FormatListNumbered
import androidx.compose.material.icons.filled.FormatQuote
import androidx.compose.material.icons.filled.FormatStrikethrough
import androidx.compose.material.icons.filled.Link
import androidx.compose.material.icons.filled.Lock
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.PushPin
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.outlined.PushPin
import androidx.compose.material.icons.outlined.StarOutline
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.InputChip
import androidx.compose.material3.LocalTextStyle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.withContext
import org.evsyukov.notes.DevicePrefs
import org.evsyukov.notes.R
import org.evsyukov.notes.Repo
import org.evsyukov.notes.editor.EditorColors
import org.evsyukov.notes.editor.Look
import org.evsyukov.notes.editor.NoteEditText
import uniffi.notes_core.MarkdownFormat
import uniffi.notes_core.Note
import uniffi.notes_core.markdownLayout

/** The note open full-screen: its title, its tags and its text, with the formats over the keyboard (R1, R20, R28, E20). */
@OptIn(ExperimentalLayoutApi::class)
@Composable
fun NoteScreen(id: String) {
    val context = LocalContext.current
    val view = remember(id) { NoteEditText(context) }
    var note by remember(id) { mutableStateOf<Note?>(null) }
    var textFocused by remember { mutableStateOf(false) }
    var findOpen by remember { mutableStateOf(false) }
    var moving by remember { mutableStateOf(false) }
    val revision = AppModel.textRevision

    LaunchedEffect(id, revision) {
        withContext(Repo.core) { runCatching { Repo.store.getNote(id) }.getOrNull() }?.let { note = it }
    }
    DisposableEffect(view) {
        view.onEdit = { edit, whole -> AppModel.textEdited(id, edit, whole) }
        view.onFocus = { textFocused = it }
        view.onOpen = AppModel::openLink
        view.onNoteFromSelection = {
            view.selectedOffsets?.let { (start, end) -> AppModel.noteFromSelection(id, start, end) }
        }
        onDispose { AppModel.flushEdits() }
    }
    // The text is laid out before it shows, so that a note does not open with its markup.
    LaunchedEffect(view) {
        val (text, layout) = withContext(Dispatchers.Default) {
            val text = withContext(Repo.core) { runCatching { Repo.store.getText(id) }.getOrDefault("") }
            text to markdownLayout(text)
        }
        view.setNote(text, layout)
        AppModel.editorShows(text)
        if (AppModel.cursorAtEnd) {
            AppModel.cursorAtEnd = false
            view.setSelection(text.length)
            view.requestFocus()
        }
    }
    // What changed in the core while no edit of this view was waiting comes in as an edit from outside (S40).
    LaunchedEffect(view, revision) {
        if (AppModel.hasPendingEdits) return@LaunchedEffect
        val shown = view.text.toString()
        val core = withContext(Repo.core) { runCatching { Repo.store.getText(id) }.getOrNull() } ?: return@LaunchedEffect
        if (view.text.toString() == shown && core != shown && view.follow(core, AppModel.hasPendingEdits)) AppModel.editorShows(core)
    }

    val locked = note?.locked == true
    val colors = editorColors()
    val settings = AppModel.settings
    val look = Look(
        settings.fontBody, settings.fontMono, DevicePrefs.textSize(context), settings.lineHeight, settings.paragraphSpacing, settings.align,
        settings.hyphenate, settings.smartTypography, settings.spellcheck, DevicePrefs.columnWidth(context).characters, colors,
    )
    LaunchedEffect(look) { view.applyLook(look) }
    LaunchedEffect(locked) { view.setEditable(!locked) }

    Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding().imePadding()) {
        val writing = textFocused && WindowInsets.isImeVisible
        TopBar(note, view, { findOpen = true }, { moving = true })
        if (locked) note?.let { LockBar(it) }
        if (findOpen) FindBar(view) { findOpen = false }
        // While the text is typed in, it has the screen that is left over the keyboard.
        if (!writing && !findOpen) note?.let {
            TitleField(it, view)
            TagRow(it)
            HorizontalDivider()
        }
        AndroidView({ view }, Modifier.weight(1f).fillMaxWidth())
        if (writing && !locked) FormatBar(view)
    }
    if (moving) {
        MoveDialog({ folder ->
            moving = false
            AppModel.moveNotes(listOf(id), folder)
        }, { moving = false })
    }
}

/** The colours of the text, from the theme. */
@Composable
private fun editorColors(): EditorColors {
    val scheme = MaterialTheme.colorScheme
    val dark = when (DevicePrefs.theme(LocalContext.current)) {
        "light" -> false
        "dark" -> true
        else -> isSystemInDarkTheme()
    }
    return EditorColors(
        scheme.onSurface.toArgb(), scheme.onSurfaceVariant.copy(alpha = 0.6f).toArgb(), scheme.primary.toArgb(), scheme.error.toArgb(),
        scheme.primary.toArgb(), scheme.onSurface.copy(alpha = 0.08f).toArgb(), Color(0x59FFC107).toArgb(), Color(0x80FF9800).toArgb(), dark,
    )
}

@Composable
private fun TopBar(note: Note?, view: NoteEditText, onFind: () -> Unit, onMove: () -> Unit) {
    var menu by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().height(56.dp), verticalAlignment = Alignment.CenterVertically) {
        IconButton({ AppModel.closeNote() }) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
        Row(Modifier.weight(1f)) {}
        if (note == null) return@Row
        IconButton({ AppModel.setFlag(NoteFlag.PINNED, !note.pinned, note.id) }) {
            Icon(if (note.pinned) Icons.Filled.PushPin else Icons.Outlined.PushPin, stringResource(if (note.pinned) R.string.unpin else R.string.pin))
        }
        IconButton({ AppModel.setFlag(NoteFlag.STARRED, !note.starred, note.id) }) {
            Icon(if (note.starred) Icons.Filled.Star else Icons.Outlined.StarOutline, stringResource(if (note.starred) R.string.remove_star else R.string.star))
        }
        IconButton({ menu = true }) { Icon(Icons.Filled.MoreVert, stringResource(R.string.more)) }
        DropdownMenu(menu, { menu = false }) {
            @Composable
            fun item(title: Int, action: () -> Unit) {
                DropdownMenuItem({ Text(stringResource(title)) }, {
                    menu = false
                    action()
                })
            }
            item(R.string.find_in_note, onFind)
            item(if (note.archived) R.string.unarchive else R.string.archive) { AppModel.setFlag(NoteFlag.ARCHIVED, !note.archived, note.id) }
            item(if (note.locked) R.string.unlock else R.string.lock) { AppModel.setFlag(NoteFlag.LOCKED, !note.locked, note.id) }
            item(R.string.move_to, onMove)
            item(R.string.duplicate) { AppModel.duplicate(note.id) }
            item(R.string.delete) { AppModel.trash(note.id) }
        }
    }
}

/** R28: a locked note opens read-only, with the way out. */
@Composable
private fun LockBar(note: Note) {
    Row(
        Modifier.fillMaxWidth().background(MaterialTheme.colorScheme.surfaceVariant).padding(horizontal = 16.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Icon(Icons.Filled.Lock, null, Modifier.size(16.dp))
        Text(stringResource(R.string.note_locked), Modifier.weight(1f).padding(horizontal = 8.dp), style = MaterialTheme.typography.bodyMedium)
        TextButton({ AppModel.setFlag(NoteFlag.LOCKED, false, note.id) }) { Text(stringResource(R.string.unlock)) }
    }
}

/** R1, R2: the title above the text; a new note has the cursor here, and Return moves it to the text. */
@Composable
private fun TitleField(note: Note, view: NoteEditText) {
    var title by remember(note.id) { mutableStateOf(note.title) }
    var focused by remember { mutableStateOf(false) }
    val focus = remember { FocusRequester() }
    LaunchedEffect(note.id) {
        if (AppModel.titleFocus) {
            AppModel.titleFocus = false
            focus.requestFocus()
        }
    }
    LaunchedEffect(note.title) { if (!focused) title = note.title }
    // A title is saved when typing pauses: each saving is an action of undo in the core.
    LaunchedEffect(title) {
        if (title == note.title) return@LaunchedEffect
        AppModel.touched(note.id)
        delay(400)
        AppModel.setTitle(note.id, title)
    }
    BasicTextField(
        title,
        { title = it },
        Modifier.fillMaxWidth().padding(16.dp, 4.dp).focusRequester(focus).onFocusChanged { focused = it.isFocused },
        enabled = !note.locked,
        singleLine = true,
        textStyle = LocalTextStyle.current.copy(fontSize = 24.sp, fontWeight = FontWeight.SemiBold, color = MaterialTheme.colorScheme.onSurface),
        cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Next),
        keyboardActions = KeyboardActions(onNext = { view.requestFocus() }),
        decorationBox = { field ->
            if (title.isEmpty()) Text(stringResource(R.string.title), fontSize = 24.sp, color = MaterialTheme.colorScheme.onSurfaceVariant)
            field()
        },
    )
}

/** R20: the tags as chips, and a field that ends a tag with Return or a comma, with completion from the existing tags. */
@OptIn(ExperimentalLayoutApi::class)
@Composable
private fun TagRow(note: Note) {
    var new by remember(note.id) { mutableStateOf("") }
    fun add() {
        val tag = new.trim()
        new = ""
        if (tag.isNotEmpty()) AppModel.addTag(note.id, tag)
    }
    FlowRow(
        Modifier.fillMaxWidth().padding(horizontal = 16.dp),
        horizontalArrangement = Arrangement.spacedBy(6.dp),
        verticalArrangement = Arrangement.Center,
    ) {
        note.tags.forEach { tag ->
            InputChip(
                false,
                { if (!note.locked) AppModel.removeTag(note.id, tag) },
                { Text(tag) },
                trailingIcon = if (note.locked) null else ({ Icon(Icons.Filled.Close, stringResource(R.string.remove_tag), Modifier.size(14.dp)) }),
            )
        }
        if (!note.locked) {
            BasicTextField(
                new,
                {
                    if (it.endsWith(",")) {
                        new = it.dropLast(1)
                        add()
                    } else {
                        new = it
                    }
                },
                Modifier.padding(vertical = 8.dp),
                singleLine = true,
                textStyle = MaterialTheme.typography.bodyMedium.copy(color = MaterialTheme.colorScheme.onSurface),
                cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Done),
                keyboardActions = KeyboardActions(onDone = { add() }),
                decorationBox = { field ->
                    if (new.isEmpty()) Text(stringResource(R.string.add_tag), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    field()
                },
            )
            val typed = new.trim().lowercase()
            if (typed.isNotEmpty()) {
                AppModel.tags.map { it.name }.filter { it.lowercase().startsWith(typed) && it !in note.tags }.take(3).forEach { tag ->
                    TextButton({
                        new = tag
                        add()
                    }) { Text(tag) }
                }
            }
        }
    }
}

/** E20: the formats over the keyboard, with E25: undo and redo of typing. */
@Composable
private fun FormatBar(view: NoteEditText) {
    @Composable
    fun button(icon: ImageVector, title: Int, enabled: Boolean = true, action: () -> Unit) {
        IconButton(action, enabled = enabled) { Icon(icon, stringResource(title)) }
    }

    @Composable
    fun heading(level: Int) {
        TextButton({ view.format(MarkdownFormat.Heading(level.toUByte())) }) { Text("H$level", fontWeight = FontWeight.Bold) }
    }
    HorizontalDivider()
    LazyRow(Modifier.fillMaxWidth().height(48.dp), verticalAlignment = Alignment.CenterVertically) {
        item { button(Icons.AutoMirrored.Filled.Undo, R.string.undo, view.canUndo) { view.undo() } }
        item { button(Icons.AutoMirrored.Filled.Redo, R.string.redo, view.canRedo) { view.redo() } }
        item { button(Icons.Filled.FormatBold, R.string.bold) { view.format(MarkdownFormat.Bold) } }
        item { button(Icons.Filled.FormatItalic, R.string.italic) { view.format(MarkdownFormat.Italic) } }
        item { button(Icons.AutoMirrored.Filled.FormatListBulleted, R.string.bulleted_list) { view.format(MarkdownFormat.BulletList) } }
        item { button(Icons.Filled.ChecklistRtl, R.string.task_list) { view.format(MarkdownFormat.TaskList) } }
        item { button(Icons.Filled.FormatListNumbered, R.string.numbered_list) { view.format(MarkdownFormat.NumberedList) } }
        item { heading(1) }
        item { heading(2) }
        item { heading(3) }
        item { button(Icons.Filled.Link, R.string.link) { view.format(MarkdownFormat.Link) } }
        item { button(Icons.Filled.Code, R.string.code) { view.format(MarkdownFormat.Code) } }
        item { button(Icons.Filled.FormatStrikethrough, R.string.strikethrough) { view.format(MarkdownFormat.Strikethrough) } }
        item { button(Icons.Filled.FormatColorFill, R.string.highlight) { view.format(MarkdownFormat.Highlight) } }
        item { button(Icons.Filled.FormatQuote, R.string.quote) { view.format(MarkdownFormat.Quote) } }
        item { button(Icons.AutoMirrored.Filled.FormatIndentDecrease, R.string.outdent) { view.indent(true) } }
        item { button(Icons.AutoMirrored.Filled.FormatIndentIncrease, R.string.indent) { view.indent(false) } }
    }
}

/** R48: find in the note, with replace. */
@Composable
private fun FindBar(view: NoteEditText, onClose: () -> Unit) {
    var query by remember { mutableStateOf("") }
    var with by remember { mutableStateOf("") }
    var count by remember { mutableStateOf(0) }
    LaunchedEffect(query) { count = view.find(query) }
    DisposableEffect(Unit) { onDispose { view.endFind() } }
    Column(Modifier.fillMaxWidth().padding(horizontal = 12.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(
                query, { query = it }, Modifier.weight(1f), singleLine = true,
                label = { Text(stringResource(R.string.find)) },
                supportingText = if (query.isEmpty()) null else ({ Text(stringResource(R.string.found_count, count)) }),
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                keyboardActions = KeyboardActions(onSearch = { view.findStep(true) }),
            )
            IconButton({ view.findStep(false) }) { Icon(Icons.Filled.ArrowUpward, stringResource(R.string.find_previous)) }
            IconButton({ view.findStep(true) }) { Icon(Icons.Filled.ArrowDownward, stringResource(R.string.find_next)) }
            IconButton(onClose) { Icon(Icons.Filled.Close, stringResource(R.string.close)) }
        }
        Row(verticalAlignment = Alignment.CenterVertically) {
            OutlinedTextField(with, { with = it }, Modifier.weight(1f), singleLine = true, label = { Text(stringResource(R.string.replace_with)) })
            TextButton({ view.replaceFound(query, with, false) }) { Text(stringResource(R.string.replace), maxLines = 1, overflow = TextOverflow.Ellipsis) }
            TextButton({ view.replaceFound(query, with, true) }) { Text(stringResource(R.string.replace_all), maxLines = 1, overflow = TextOverflow.Ellipsis) }
        }
    }
}
