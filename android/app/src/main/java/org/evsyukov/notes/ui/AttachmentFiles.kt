package org.evsyukov.notes.ui

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.media.MediaPlayer
import android.net.Uri
import android.provider.OpenableColumns
import androidx.core.content.FileProvider
import org.evsyukov.notes.logError
import org.evsyukov.notes.logged
import uniffi.notes_core.Attachment
import java.io.File

/** Attachments on their way in and out of the app. The content lives under the hash of its bytes, without a name. */
object AttachmentFiles {
    /** Copies what a content URI points at into a cache file named like the original; a file that does not come is logged (R95). */
    fun copyToCache(context: Context, uri: Uri): File? = runCatching {
        val name = context.contentResolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { cursor ->
            if (cursor.moveToFirst()) cursor.getString(0) else null
        } ?: uri.lastPathSegment ?: "file"
        val dir = File(context.cacheDir, "incoming/${System.nanoTime()}").apply { mkdirs() }
        val target = File(dir, name.replace('/', '_'))
        context.contentResolver.openInputStream(uri)?.use { input -> target.outputStream().use(input::copyTo) }
            ?: run {
                logError("attachments", "a picked or shared file was not copied: the provider gave no content")
                return null
            }
        target
    }.onFailure { logError("attachments", "a picked or shared file was not copied: ${logged(it)}") }.getOrNull()

    private fun uri(context: Context, file: Attachment): Uri? = file.localPath?.let {
        FileProvider.getUriForFile(context, "${context.packageName}.files", File(it), file.name)
    }

    /** R32: the system app for the file, through `FileProvider`. */
    fun open(context: Context, file: Attachment) {
        runCatching {
            val uri = uri(context, file) ?: return
            val intent = Intent(Intent.ACTION_VIEW).setDataAndType(uri, file.mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            context.startActivity(Intent.createChooser(intent, file.name))
        }.onFailure { logError("attachments", "a file was not opened: ${logged(it)}") }
    }

    fun share(context: Context, file: Attachment) {
        runCatching {
            val uri = uri(context, file) ?: return
            val intent = Intent(Intent.ACTION_SEND).setType(file.mime).putExtra(Intent.EXTRA_STREAM, uri)
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
            intent.clipData = ClipData.newRawUri(null, uri)
            context.startActivity(Intent.createChooser(intent, file.name))
        }.onFailure { logError("attachments", "a file was not shared: ${logged(it)}") }
    }
}

/** R31: the sound of an attachment, played in its line. One at a time. */
object AudioPlayer {
    private var player: MediaPlayer? = null

    var playing: String? = null
        private set

    /** Starts or stops [file]; [changed] hears when it stops by itself. */
    fun toggle(file: Attachment, changed: () -> Unit) {
        val same = playing == file.id
        stop()
        if (same) return
        val path = file.localPath ?: return
        runCatching {
            player = MediaPlayer().apply {
                setDataSource(path)
                setOnCompletionListener {
                    stop()
                    changed()
                }
                prepare()
                start()
            }
            playing = file.id
        }.onFailure { logError("attachments", "a sound was not played: ${logged(it)}") }
    }

    fun stop() {
        player?.release()
        player = null
        playing = null
    }
}
