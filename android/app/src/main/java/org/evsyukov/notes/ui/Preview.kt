package org.evsyukov.notes.ui

import android.graphics.Bitmap
import android.graphics.pdf.PdfRenderer
import android.net.Uri
import android.os.ParcelFileDescriptor
import android.widget.MediaController
import android.widget.VideoView
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectTransformGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.OpenInNew
import androidx.compose.material.icons.outlined.Close
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import org.evsyukov.notes.R
import org.evsyukov.notes.editor.AttachmentKind
import org.evsyukov.notes.editor.attachmentKind
import org.evsyukov.notes.editor.decodeImage
import uniffi.notes_core.Attachment
import java.io.File
import kotlin.math.max
import android.graphics.Color as AndroidColor

/** An image, a PDF or a video over the whole screen (R31). The file can also go to another app. */
@Composable
fun AttachmentViewer(file: Attachment, onClose: () -> Unit) {
    val path = file.localPath ?: return
    val context = LocalContext.current
    Dialog(onDismissRequest = onClose, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        Column(Modifier.fillMaxSize().background(Color.Black).systemBarsPadding()) {
            Row(Modifier.fillMaxWidth().padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(file.name, Modifier.weight(1f), color = Color.White, maxLines = 1, overflow = TextOverflow.Ellipsis)
                IconButton({ AttachmentFiles.share(context, file) }) { Icon(Icons.Outlined.Share, stringResource(R.string.share), tint = Color.White) }
                IconButton({ AttachmentFiles.open(context, file) }) { Icon(Icons.AutoMirrored.Outlined.OpenInNew, stringResource(R.string.open_in_another_app), tint = Color.White) }
                IconButton(onClose) { Icon(Icons.Outlined.Close, stringResource(R.string.close), tint = Color.White) }
            }
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                when (attachmentKind(file)) {
                    AttachmentKind.PDF -> PdfPages(path)
                    AttachmentKind.VIDEO -> VideoPlayer(path)
                    else -> ZoomableImage(path)
                }
            }
        }
    }
}

@Composable
private fun CannotShow() {
    Text(stringResource(R.string.cannot_show_file), Modifier.padding(24.dp), color = Color.White)
}

/** R31: the player of a video, with the controls of the system. */
@Composable
private fun VideoPlayer(path: String) {
    AndroidView({ context ->
        VideoView(context).apply {
            val controls = MediaController(context)
            controls.setAnchorView(this)
            setMediaController(controls)
            setVideoURI(Uri.fromFile(File(path)))
            setOnPreparedListener { start() }
            setOnErrorListener { _, _, _ -> true }
        }
    }, Modifier.fillMaxSize())
}

/** The image fitted to the screen; two fingers and a double tap zoom it, a drag moves it while zoomed. */
@Composable
private fun ZoomableImage(path: String) {
    BoxWithConstraints(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
        val side = max(constraints.maxWidth, constraints.maxHeight)
        val loaded by produceState<Result<Bitmap?>?>(null, path, side) {
            value = Result.success(withContext(Dispatchers.IO) { decodeImage(path, side) })
        }
        val bitmap = loaded?.getOrNull()
        if (loaded != null && bitmap == null) return@BoxWithConstraints CannotShow()
        if (bitmap == null) return@BoxWithConstraints

        var scale by remember(path) { mutableFloatStateOf(1f) }
        var offset by remember(path) { mutableStateOf(Offset.Zero) }
        var size by remember { mutableStateOf(IntSize.Zero) }

        // The picture never leaves the screen: it can be moved only by what the zoom added.
        fun held(to: Offset, at: Float): Offset {
            val x = size.width * (at - 1f) / 2f
            val y = size.height * (at - 1f) / 2f
            return Offset(to.x.coerceIn(-x, x), to.y.coerceIn(-y, y))
        }
        Image(
            bitmap.asImageBitmap(), null,
            Modifier.fillMaxSize().onSizeChanged { size = it }
                .pointerInput(path) {
                    detectTapGestures(onDoubleTap = {
                        if (scale > 1f) {
                            scale = 1f
                            offset = Offset.Zero
                        } else {
                            scale = 2.5f
                        }
                    })
                }
                .pointerInput(path) {
                    detectTransformGestures { _, pan, zoom, _ ->
                        scale = (scale * zoom).coerceIn(1f, 8f)
                        offset = held(offset + pan, scale)
                    }
                }
                .graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y),
            contentScale = ContentScale.Fit,
        )
    }
}

/** A PDF opened with the renderer of the platform. It draws one page at a time and may not be closed while it draws, so both go through one lock. */
private class PdfFile(path: String) {
    private val renderer = PdfRenderer(ParcelFileDescriptor.open(File(path), ParcelFileDescriptor.MODE_READ_ONLY))
    private var closed = false
    val pages: Int = renderer.pageCount

    @Synchronized
    fun page(index: Int, width: Int): Bitmap? {
        if (closed) return null
        return renderer.openPage(index).use { page ->
            val height = max(1, width * page.height / max(1, page.width))
            Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888).also {
                it.eraseColor(AndroidColor.WHITE)
                page.render(it, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
            }
        }
    }

    @Synchronized
    fun close() {
        if (!closed) renderer.close()
        closed = true
    }
}

/** The pages one under another, each drawn when it comes on screen. */
@Composable
private fun PdfPages(path: String) {
    // A file that is not a PDF, or one that asks for a password, cannot be opened.
    val pdf = remember(path) { runCatching { PdfFile(path) }.getOrNull() }
    DisposableEffect(pdf) { onDispose { pdf?.close() } }
    if (pdf == null) return CannotShow()
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val width = constraints.maxWidth.coerceIn(1, 2048)
        LazyColumn(Modifier.fillMaxSize(), verticalArrangement = Arrangement.spacedBy(8.dp)) {
            items(pdf.pages) { index ->
                val page by produceState<Bitmap?>(null, pdf, index, width) {
                    value = withContext(Dispatchers.IO) { runCatching { pdf.page(index, width) }.getOrNull() }
                }
                val bitmap = page
                if (bitmap != null) {
                    Image(bitmap.asImageBitmap(), "${index + 1} / ${pdf.pages}", Modifier.fillMaxWidth(), contentScale = ContentScale.FillWidth)
                } else {
                    // Roughly a sheet of paper, so the list does not jump when the page arrives.
                    Box(Modifier.fillMaxWidth().aspectRatio(0.707f).background(Color.White))
                }
            }
        }
    }
}
