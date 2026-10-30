package org.evsyukov.notes.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.outlined.RemoveCircleOutline
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import org.evsyukov.notes.R
import uniffi.notes_core.Condition
import uniffi.notes_core.DateTest
import uniffi.notes_core.FilterJoin
import uniffi.notes_core.FilterQuery
import uniffi.notes_core.Folder
import uniffi.notes_core.SavedFilter

/** A saved filter while it is edited (R45). */
data class FilterDraft(
    val filter: String? = null,
    val name: String = "",
    val icon: String = "",
    val join: FilterJoin = FilterJoin.ALL,
    val conditions: List<Condition> = emptyList(),
) {
    constructor(saved: SavedFilter) : this(saved.id, saved.name, saved.icon, saved.query.join, saved.query.conditions)
}

/** The kinds of condition of R45, each with what the dialog asks for. */
enum class ConditionKind(val title: Int, val takesText: Boolean = false) {
    IN_FOLDER(R.string.condition_in_folder),
    HAS_TAG(R.string.condition_has_tag, true),
    NO_TAGS(R.string.condition_no_tags),
    NONE_OF_TAGS(R.string.condition_none_of_tags, true),
    PINNED(R.string.condition_pinned),
    STARRED(R.string.condition_starred),
    ARCHIVED(R.string.condition_archived),
    HAS_ATTACHMENTS(R.string.condition_has_attachments),
    TITLE_IS(R.string.condition_title_is, true),
    TITLE_CONTAINS(R.string.condition_title_contains, true),
    TEXT_CONTAINS(R.string.condition_text_contains, true),
    CREATED_WITHIN(R.string.condition_created_within, true),
    EDITED_WITHIN(R.string.condition_edited_within, true);

    /** The condition of this kind with [value] as its text, or a default one. */
    fun condition(value: String, folders: List<Folder>): Condition = when (this) {
        IN_FOLDER -> Condition.InFolder(folders.firstOrNull { it.id == value }?.id ?: folders.firstOrNull()?.id.orEmpty(), true)
        HAS_TAG -> Condition.HasTag(value)
        NO_TAGS -> Condition.NoTags
        NONE_OF_TAGS -> Condition.NoneOfTags(value.split(",").map { it.trim() })
        PINNED -> Condition.Pinned
        STARRED -> Condition.Starred
        ARCHIVED -> Condition.Archived
        HAS_ATTACHMENTS -> Condition.HasAttachments
        TITLE_IS -> Condition.TitleIs(value)
        TITLE_CONTAINS -> Condition.TitleContains(value)
        TEXT_CONTAINS -> Condition.TextContains(value)
        CREATED_WITHIN -> Condition.Created(DateTest.Within(value.toUIntOrNull() ?: 7u))
        EDITED_WITHIN -> Condition.Edited(DateTest.Within(value.toUIntOrNull() ?: 7u))
    }

    companion object {
        fun of(condition: Condition): ConditionKind = when (condition) {
            is Condition.InFolder -> IN_FOLDER
            is Condition.HasTag -> HAS_TAG
            Condition.NoTags -> NO_TAGS
            is Condition.NoneOfTags -> NONE_OF_TAGS
            Condition.Pinned -> PINNED
            Condition.Starred -> STARRED
            Condition.Archived -> ARCHIVED
            Condition.HasAttachments -> HAS_ATTACHMENTS
            is Condition.TitleIs -> TITLE_IS
            is Condition.TitleContains -> TITLE_CONTAINS
            is Condition.TextContains -> TEXT_CONTAINS
            is Condition.Created -> CREATED_WITHIN
            is Condition.Edited -> EDITED_WITHIN
        }

        fun value(condition: Condition): String = when (condition) {
            is Condition.InFolder -> condition.folderId
            is Condition.HasTag -> condition.name
            is Condition.NoneOfTags -> condition.names.joinToString(", ")
            is Condition.TitleIs -> condition.text
            is Condition.TitleContains -> condition.text
            is Condition.TextContains -> condition.text
            is Condition.Created -> ((condition.`when` as? DateTest.Within)?.days ?: 7u).toString()
            is Condition.Edited -> ((condition.`when` as? DateTest.Within)?.days ?: 7u).toString()
            else -> ""
        }
    }
}

/** R45: the name, the icon and the conditions of a saved filter. */
@Composable
fun FilterEditor(initial: FilterDraft, onDismiss: () -> Unit) {
    var draft by remember { mutableStateOf(initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        text = {
            Column(Modifier.heightIn(max = 480.dp).verticalScroll(rememberScrollState())) {
                OutlinedTextField(draft.name, { draft = draft.copy(name = it) }, label = { Text(stringResource(R.string.name)) }, singleLine = true)
                OutlinedTextField(draft.icon, { draft = draft.copy(icon = it) }, label = { Text(stringResource(R.string.icon_emoji)) }, singleLine = true, modifier = Modifier.padding(top = 8.dp))
                Picker(
                    stringResource(if (draft.join == FilterJoin.ALL) R.string.join_all else R.string.join_any),
                    listOf(stringResource(R.string.join_all) to FilterJoin.ALL, stringResource(R.string.join_any) to FilterJoin.ANY),
                ) { draft = draft.copy(join = it) }
                draft.conditions.forEachIndexed { index, condition ->
                    ConditionRow(condition, { draft = draft.copy(conditions = draft.conditions.toMutableList().apply { this[index] = it }) }) {
                        draft = draft.copy(conditions = draft.conditions.toMutableList().apply { removeAt(index) })
                    }
                }
                TextButton({ draft = draft.copy(conditions = draft.conditions + Condition.TitleContains("")) }) { Text(stringResource(R.string.add_condition)) }
            }
        },
        confirmButton = {
            TextButton({
                onDismiss()
                AppModel.saveFilter(draft.filter, draft.name, draft.icon, FilterQuery(draft.join, draft.conditions))
            }, enabled = draft.name.isNotBlank() && draft.conditions.isNotEmpty()) { Text(stringResource(R.string.save)) }
        },
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
private fun ConditionRow(condition: Condition, onChange: (Condition) -> Unit, onRemove: () -> Unit) {
    val kind = ConditionKind.of(condition)
    val folders = AppModel.folders
    Column(Modifier.fillMaxWidth().padding(top = 8.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Picker(stringResource(kind.title), ConditionKind.entries.map { stringResource(it.title) to it }) { onChange(it.condition("", folders)) }
            }
            IconButton(onRemove) { Icon(Icons.Outlined.RemoveCircleOutline, stringResource(R.string.remove)) }
        }
        if (kind == ConditionKind.IN_FOLDER) {
            val current = ConditionKind.value(condition)
            Picker(
                AppModel.path(current).joinToString(" / ") { it.name },
                folders.map { folder -> AppModel.path(folder.id).joinToString(" / ") { it.name } to folder.id },
            ) { onChange(kind.condition(it, folders)) }
        } else if (kind.takesText) {
            OutlinedTextField(ConditionKind.value(condition), { onChange(kind.condition(it, folders)) }, singleLine = true, modifier = Modifier.fillMaxWidth())
        }
    }
}

/** The current choice as a line; a tap lists the others. */
@Composable
fun <T> Picker(current: String, choices: List<Pair<String, T>>, onChoose: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Text(current, Modifier.fillMaxWidth().clickable { open = true }.padding(vertical = 10.dp))
    DropdownMenu(open, { open = false }) {
        choices.forEach { (title, value) ->
            DropdownMenuItem({ Text(title) }, {
                open = false
                onChoose(value)
            })
        }
    }
}
