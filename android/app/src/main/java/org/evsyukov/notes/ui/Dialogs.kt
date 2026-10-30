package org.evsyukov.notes.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.unit.dp
import org.evsyukov.notes.R
import org.evsyukov.notes.ui.AppModel.path

/** A question with a confirming button. */
@Composable
fun ConfirmDialog(text: String, confirm: String, onConfirm: () -> Unit, onDismiss: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDismiss,
        text = { Text(text) },
        confirmButton = {
            TextButton({
                onDismiss()
                onConfirm()
            }) { Text(confirm) }
        },
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** One or two lines of text to enter, with Save. The first must not be empty. */
@Composable
fun TextDialog(title: String, label: String, initial: String, onSave: (String, String) -> Unit, onDismiss: () -> Unit, secondLabel: String? = null, secondInitial: String = "") {
    var first by remember { mutableStateOf(initial) }
    var second by remember { mutableStateOf(secondInitial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(title) },
        text = {
            Column {
                OutlinedTextField(first, { first = it }, label = { Text(label) }, singleLine = true)
                if (secondLabel != null) {
                    OutlinedTextField(second, { second = it }, label = { Text(secondLabel) }, singleLine = true, modifier = Modifier.padding(top = 8.dp))
                }
            }
        },
        confirmButton = {
            TextButton({
                onDismiss()
                onSave(first, second)
            }, enabled = first.isNotBlank()) { Text(stringResource(R.string.save)) }
        },
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** R11: "Move to…": the tree of folders, or no folder. [hidden] leaves out a folder that cannot be the target. */
@Composable
fun MoveDialog(onMove: (String?) -> Unit, onDismiss: () -> Unit, hidden: (String) -> Boolean = { false }) {
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.move_to)) },
        text = {
            LazyColumn(Modifier.heightIn(max = 400.dp)) {
                item { MoveRow(stringResource(R.string.no_folder)) { onMove(null) } }
                items(AppModel.folders.size) { index ->
                    val folder = AppModel.folders[index]
                    if (!hidden(folder.id)) MoveRow(path(folder.id).joinToString(" / ") { it.name }) { onMove(folder.id) }
                }
            }
        },
        confirmButton = {},
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
private fun MoveRow(title: String, onClick: () -> Unit) {
    Text(title, Modifier.fillMaxWidth().clickable(onClick = onClick).padding(vertical = 12.dp))
}
