package org.evsyukov.notes.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.style.TextDecoration
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import org.evsyukov.notes.R
import uniffi.notes_core.Version
import uniffi.notes_core.VersionDiff
import java.text.DateFormat
import java.util.Date

/** R40–R43: the versions of the text of the open note: the list, a version with what differs from the current text marked, and the way back to it. */
@Composable
fun VersionHistory(id: String, locked: Boolean, onClose: () -> Unit) {
    var versions by remember { mutableStateOf<List<Version>?>(null) }
    var chosen by remember { mutableStateOf<Version?>(null) }
    var diff by remember { mutableStateOf<VersionDiff?>(null) }
    LaunchedEffect(id) { AppModel.versions(id) { versions = it } }
    LaunchedEffect(chosen) {
        diff = null
        chosen?.let { version -> AppModel.versionDiff(id, version.id) { diff = it } }
    }
    fun back() {
        if (chosen != null) chosen = null else onClose()
    }
    Dialog(::back, DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        BackHandler(onBack = ::back)
        Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background).statusBarsPadding().navigationBarsPadding()) {
            Row(Modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
                IconButton(::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
                Text(stringResource(R.string.version_history), style = MaterialTheme.typography.titleLarge)
            }
            val version = chosen
            val text = diff
            if (version == null) {
                Box(Modifier.fillMaxSize()) {
                    LazyColumn(Modifier.fillMaxSize()) {
                        items(versions.orEmpty(), key = { it.id }) { VersionRow(it) { chosen = it } }
                    }
                    if (versions?.isEmpty() == true) {
                        Text(stringResource(R.string.no_versions), Modifier.align(Alignment.Center), color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            } else if (text != null) {
                SelectionContainer(Modifier.weight(1f).fillMaxWidth().verticalScroll(rememberScrollState()).padding(16.dp)) { Text(marked(text)) }
                HorizontalDivider()
                Button({
                    AppModel.restoreVersion(id, version.id)
                    onClose()
                }, Modifier.align(Alignment.End).padding(12.dp), enabled = !locked) { Text(stringResource(R.string.restore_version)) }
            }
        }
    }
}

@Composable
private fun VersionRow(version: Version, onClick: () -> Unit) {
    Column(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(16.dp, 10.dp)) {
        Text(DateFormat.getDateTimeInstance(DateFormat.MEDIUM, DateFormat.SHORT).format(Date(version.timeMs.toLong())))
        Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            val device = version.device.ifEmpty { stringResource(if (version.thisDevice) R.string.this_device else R.string.another_device) }
            Text(device, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Text("+${version.added}", style = MaterialTheme.typography.bodySmall, color = Color(0xFF2E7D32))
            Text("−${version.removed}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.error)
        }
    }
    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
}

/** R41: read-only, with what the current text does not have on a coloured ground. A place where the current text has more is marked by the character before it. */
private fun marked(diff: VersionDiff): AnnotatedString = buildAnnotatedString {
    append(diff.text)
    diff.changed.forEach { addStyle(SpanStyle(background = Color(0x4D4CAF50)), it.start.toInt(), it.end.toInt().coerceAtMost(diff.text.length)) }
    diff.removedAt.filter { it > 0u && it.toInt() <= diff.text.length }.forEach {
        addStyle(SpanStyle(color = Color.Red, textDecoration = TextDecoration.Underline), it.toInt() - 1, it.toInt())
    }
}
