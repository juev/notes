package org.evsyukov.notes.ui

import android.content.Context
import android.net.Uri
import android.print.PrintAttributes
import android.print.PrintManager
import android.provider.DocumentsContract
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Checkbox
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import org.evsyukov.notes.R
import org.evsyukov.notes.logError
import org.evsyukov.notes.logged
import uniffi.notes_core.ExportScope
import uniffi.notes_core.ImportOptions
import java.io.File

/** Moves whole folders between the cache of the app, where the core works, and a folder the user picked. */
object FolderCopy {
    /** Writes the files under [from] into the tree the user picked; folders stay folders. */
    fun toTree(context: Context, from: File, tree: Uri) {
        val root = DocumentsContract.buildDocumentUriUsingTree(tree, DocumentsContract.getTreeDocumentId(tree))
        write(context, from, root)
    }

    private fun write(context: Context, from: File, into: Uri) {
        val resolver = context.contentResolver
        from.listFiles().orEmpty().forEach { file ->
            if (file.isDirectory) {
                val folder = DocumentsContract.createDocument(resolver, into, DocumentsContract.Document.MIME_TYPE_DIR, file.name) ?: return@forEach
                write(context, file, folder)
            } else {
                val made = DocumentsContract.createDocument(resolver, into, "application/octet-stream", file.name) ?: return@forEach
                resolver.openOutputStream(made, "w")?.use { out -> file.inputStream().use { it.copyTo(out) } }
            }
        }
    }

    /** Reads the files of the tree the user picked into [to], for the folder of files of an import. */
    fun fromTree(context: Context, tree: Uri, to: File) {
        read(context, tree, DocumentsContract.getTreeDocumentId(tree), to)
    }

    private fun read(context: Context, tree: Uri, document: String, to: File) {
        to.mkdirs()
        val resolver = context.contentResolver
        val children = DocumentsContract.buildChildDocumentsUriUsingTree(tree, document)
        val columns = arrayOf(DocumentsContract.Document.COLUMN_DOCUMENT_ID, DocumentsContract.Document.COLUMN_DISPLAY_NAME, DocumentsContract.Document.COLUMN_MIME_TYPE)
        resolver.query(children, columns, null, null, null)?.use { cursor ->
            while (cursor.moveToNext()) {
                val id = cursor.getString(0)
                val name = cursor.getString(1).replace('/', '_')
                if (cursor.getString(2) == DocumentsContract.Document.MIME_TYPE_DIR) {
                    read(context, tree, id, File(to, name))
                } else {
                    resolver.openInputStream(DocumentsContract.buildDocumentUriUsingTree(tree, id))?.use { input ->
                        File(to, name).outputStream().use(input::copyTo)
                    }
                }
            }
        }
    }
}

/** What can be exported, with the folder to write it to still to be asked for. */
sealed class ExportJob {
    data class Markdown(val scope: ExportScope) : ExportJob()
    data class Json(val attachments: Boolean) : ExportJob()
    data class Html(val note: String, val embed: Boolean) : ExportJob()
}

/** R60–R62: a chooser of a folder that runs the job asked for. */
@Composable
fun rememberExport(): (ExportJob) -> Unit {
    val context = LocalContext.current
    var job by remember { mutableStateOf<ExportJob?>(null) }
    val pick = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { tree ->
        val asked = job
        job = null
        if (tree != null && asked != null) AppModel.export(context, asked, tree)
    }
    return { asked ->
        job = asked
        pick.launch(null)
    }
}

/** R61: the note in the print dialog of the system, from the HTML of the core; there it is saved as a PDF on A4 or Letter. */
fun printNote(context: Context, html: File, title: String) {
    val view = WebView(context)
    view.settings.allowFileAccess = true
    view.webViewClient = object : WebViewClient() {
        override fun onPageFinished(page: WebView, url: String?) {
            val manager = context.getSystemService(PrintManager::class.java)
            val attributes = PrintAttributes.Builder().setMediaSize(PrintAttributes.MediaSize.ISO_A4).build()
            runCatching { manager.print(title, page.createPrintDocumentAdapter(title), attributes) }
                .onFailure { logError("export", "the note was not printed: ${logged(it)}") }
        }
    }
    view.loadUrl(Uri.fromFile(html).toString())
}

/** R65: the options of an import, for the backup that was chosen. */
@Composable
fun ImportDialog(onImport: (Uri?, ImportOptions) -> Unit, onDismiss: () -> Unit) {
    var underOne by remember { mutableStateOf(false) }
    var collect by remember { mutableStateOf(false) }
    var files by remember { mutableStateOf<Uri?>(null) }
    val pickFiles = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocumentTree()) { files = it }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.import_title)) },
        text = {
            Column {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(underOne, { underOne = it })
                    Text(stringResource(R.string.import_under_one))
                }
                TextButton({ pickFiles.launch(null) }) { Text(stringResource(R.string.import_files_folder)) }
                Text(files?.lastPathSegment ?: stringResource(R.string.import_files_hint))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Checkbox(collect, { collect = it }, enabled = files != null)
                    Text(stringResource(R.string.import_collect))
                }
            }
        },
        confirmButton = {
            TextButton({
                onDismiss()
                onImport(files, ImportOptions(underOne, collect))
            }) { Text(stringResource(R.string.import_action)) }
        },
        dismissButton = { TextButton(onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

/** I12: the report of an import, which can be copied as text. */
@Composable
fun ImportReportDialog(text: String, onDismiss: () -> Unit) {
    val clipboard = LocalClipboardManager.current
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(stringResource(R.string.import_finished)) },
        text = { SelectionContainer(Modifier.heightIn(max = 420.dp).verticalScroll(rememberScrollState())) { Text(text) } },
        confirmButton = { TextButton(onDismiss) { Text(stringResource(R.string.close)) } },
        dismissButton = { TextButton({ clipboard.setText(AnnotatedString(text)) }) { Text(stringResource(R.string.copy)) } },
    )
}
