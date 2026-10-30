package org.evsyukov.notes.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.FilterChip
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import org.evsyukov.notes.R
import org.evsyukov.notes.shownTitle
import uniffi.notes_core.MarkdownRange
import uniffi.notes_core.SearchHit

/** R47: the notes found, the best first, with what was found marked. */
@Composable
fun SearchScreen(onClose: () -> Unit) {
    var query by remember { mutableStateOf("") }
    var archived by remember { mutableStateOf(false) }
    var trash by remember { mutableStateOf(false) }
    var hits by remember { mutableStateOf<List<SearchHit>>(emptyList()) }
    val focus = remember { FocusRequester() }
    BackHandler(onBack = onClose)
    LaunchedEffect(Unit) { focus.requestFocus() }
    LaunchedEffect(query, archived, trash) {
        if (query.isBlank()) hits = emptyList() else AppModel.search(query, archived, trash) { hits = it }
    }
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(onClose) { Icon(Icons.AutoMirrored.Filled.ArrowBack, stringResource(R.string.back)) }
            OutlinedTextField(
                query, { query = it }, Modifier.weight(1f).focusRequester(focus), singleLine = true,
                placeholder = { Text(stringResource(R.string.search)) },
                keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
            )
        }
        Row(Modifier.padding(horizontal = 12.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            FilterChip(archived, { archived = !archived }, { Text(stringResource(R.string.include_archived)) })
            FilterChip(trash, { trash = !trash }, { Text(stringResource(R.string.include_trash)) })
        }
        Box(Modifier.fillMaxSize()) {
            LazyColumn(Modifier.fillMaxSize()) {
                items(hits, key = { it.note.id }) { hit ->
                    Column(Modifier.fillMaxWidth().clickable { AppModel.open(hit.note.id) }.padding(16.dp, 10.dp)) {
                        val ranges = if (hit.note.title.isEmpty()) emptyList() else hit.titleMatches
                        Text(marked(hit.note.shownTitle(stringResource(R.string.untitled)), ranges), fontWeight = FontWeight.Medium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                        if (hit.snippet.isNotEmpty()) {
                            Text(
                                marked(hit.snippet, hit.snippetMatches), style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis,
                            )
                        }
                    }
                    HorizontalDivider(Modifier.padding(horizontal = 16.dp))
                }
            }
            if (hits.isEmpty() && query.isNotBlank()) {
                Text(stringResource(R.string.nothing_found), Modifier.align(Alignment.Center), color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
    }
}

private fun marked(text: String, ranges: List<MarkdownRange>): AnnotatedString = buildAnnotatedString {
    append(text)
    ranges.forEach {
        val end = it.end.toInt().coerceAtMost(text.length)
        if (it.start.toInt() < end) addStyle(SpanStyle(background = Color(0x66FFC107)), it.start.toInt(), end)
    }
}
