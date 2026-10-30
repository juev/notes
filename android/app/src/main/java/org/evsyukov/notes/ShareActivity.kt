package org.evsyukov.notes

import android.app.Activity
import android.app.PendingIntent
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.service.quicksettings.TileService
import kotlinx.coroutines.launch
import org.evsyukov.notes.ui.AttachmentFiles
import uniffi.notes_core.NewNote

/**
 * R86: what is shared into the app becomes a new note: the text, and each file as an attachment with its
 * link in the text. It shows nothing of its own and stays until the files are copied: the note opens in the main screen.
 */
class ShareActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val intent = intent
        val text = intent.getCharSequenceExtra(Intent.EXTRA_TEXT)?.toString().orEmpty()
        val title = intent.getStringExtra(Intent.EXTRA_SUBJECT).orEmpty()
        val uris = streams(intent)
        val context = applicationContext
        Repo.scope.launch(Repo.core) {
            val store = Repo.store
            val note = runCatching { store.createNote(NewNote(title, null, emptyList(), text)) }
                .onFailure { logError("share", "a shared note was not made: ${logged(it)}") }
                .getOrNull()
            if (note == null) {
                finish()
                return@launch
            }
            val links = uris.mapNotNull { uri ->
                val file = AttachmentFiles.copyToCache(context, uri) ?: return@mapNotNull null
                val added = runCatching { store.addAttachment(note.id, file.absolutePath, file.name) }
                    .onFailure { logError("share", "a shared file was not attached: ${logged(it)}") }.getOrNull()
                file.parentFile?.deleteRecursively()
                added?.let(AttachmentLink::text)
            }
            if (links.isNotEmpty()) {
                val body = (listOf(text).filter { it.isNotEmpty() } + links).joinToString("\n\n") + "\n"
                runCatching { store.setText(note.id, body) }
            }
            Repo.changed()
            startActivity(Intent(context, MainActivity::class.java).putExtra(MainActivity.EXTRA_NOTE, note.id).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
            // Only now: the right to read a shared file ends with this activity.
            finish()
        }
    }

    private fun streams(intent: Intent): List<Uri> {
        if (intent.action == Intent.ACTION_SEND_MULTIPLE) {
            return if (Build.VERSION.SDK_INT >= 33) {
                intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java).orEmpty()
            } else {
                @Suppress("DEPRECATION")
                intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM).orEmpty()
            }
        }
        val one = if (Build.VERSION.SDK_INT >= 33) {
            intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
        } else {
            @Suppress("DEPRECATION")
            intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM)
        }
        return listOfNotNull(one)
    }
}

/** R86: the Quick Settings tile that creates a new note. */
class QuickTileService : TileService() {
    override fun onClick() {
        val intent = Intent(this, MainActivity::class.java).setAction(MainActivity.ACTION_NEW_NOTE).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        if (Build.VERSION.SDK_INT >= 34) {
            startActivityAndCollapse(PendingIntent.getActivity(this, 0, intent, PendingIntent.FLAG_IMMUTABLE))
        } else {
            @Suppress("DEPRECATION", "StartActivityAndCollapseDeprecated")
            startActivityAndCollapse(intent)
        }
    }
}
