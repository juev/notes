package org.evsyukov.notes.editor

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Matrix
import android.graphics.Paint
import android.graphics.Path
import android.graphics.RectF
import android.graphics.pdf.PdfRenderer
import android.media.ExifInterface
import android.media.MediaMetadataRetriever
import android.os.ParcelFileDescriptor
import android.text.TextPaint
import android.text.TextUtils
import android.text.format.Formatter
import android.text.style.ReplacementSpan
import android.util.LruCache
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.evsyukov.notes.R
import uniffi.notes_core.Attachment
import java.io.File
import kotlin.math.ceil
import kotlin.math.max
import kotlin.math.min

/** What stands in the text for an attachment (E14, R31, R32, R35). */
enum class AttachmentKind { IMAGE, VIDEO, AUDIO, PDF, CHIP, PENDING, BROKEN }

fun attachmentKind(file: Attachment?): AttachmentKind = when {
    file == null -> AttachmentKind.BROKEN
    file.localPath == null -> AttachmentKind.PENDING
    file.mime.startsWith("video/") -> AttachmentKind.VIDEO
    file.mime.startsWith("audio/") -> AttachmentKind.AUDIO
    file.mime == "application/pdf" -> AttachmentKind.PDF
    file.mime.startsWith("image/") && file.mime != "image/svg+xml" -> AttachmentKind.IMAGE
    else -> AttachmentKind.CHIP
}

/**
 * Reads an image no larger than needed: the longer side of the result is at least [side] pixels when
 * the file has that many, and less than twice that. A photo is turned the way its EXIF says.
 */
fun decodeImage(path: String, side: Int): Bitmap? = runCatching {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeFile(path, bounds)
    if (bounds.outWidth <= 0 || bounds.outHeight <= 0) return null
    var sample = 1
    while (max(bounds.outWidth, bounds.outHeight) / (sample * 2) >= side) sample *= 2
    val bitmap = BitmapFactory.decodeFile(path, BitmapFactory.Options().apply { inSampleSize = sample }) ?: return null
    val degrees = when (ExifInterface(path).getAttributeInt(ExifInterface.TAG_ORIENTATION, ExifInterface.ORIENTATION_NORMAL)) {
        ExifInterface.ORIENTATION_ROTATE_90 -> 90f
        ExifInterface.ORIENTATION_ROTATE_180 -> 180f
        ExifInterface.ORIENTATION_ROTATE_270 -> 270f
        else -> 0f
    }
    if (degrees == 0f) bitmap else Bitmap.createBitmap(bitmap, 0, 0, bitmap.width, bitmap.height, Matrix().apply { postRotate(degrees) }, true)
}.getOrNull()

/** The pictures of the attachments of the open note, read off the main thread; [loaded] hears when one has come. */
class AttachmentPictures(private val context: Context, private val loaded: (String) -> Unit) {
    private class Picture(val bitmap: Bitmap?, val pages: Int)

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private val cache = object : LruCache<String, Picture>(64 * 1024 * 1024) {
        override fun sizeOf(key: String, value: Picture) = maxOf(1, value.bitmap?.allocationByteCount ?: 1)
    }
    private val asked = HashSet<String>()

    var files: List<Attachment> = emptyList()

    /** The span of an attachment for a column of this width; [spacing] is the line spacing of the view, which will multiply its height. */
    fun span(id: String, column: Int, spacing: Float, colors: EditorColors, playing: Boolean): ReplacementSpan {
        val file = files.firstOrNull { it.id == id }
        val kind = attachmentKind(file)
        val density = context.resources.displayMetrics.density
        val width = max(column, (80 * density).toInt())
        val picture = if (file != null && (kind == AttachmentKind.IMAGE || kind == AttachmentKind.PDF || kind == AttachmentKind.VIDEO)) picture(file, kind, width) else null
        val bitmap = picture?.bitmap
        val (w, h) = when (kind) {
            AttachmentKind.IMAGE, AttachmentKind.PDF -> if (bitmap != null) {
                val scale = min(1f, width.toFloat() / bitmap.width)
                ceil(bitmap.width * scale).toInt() to ceil(bitmap.height * scale).toInt()
            } else {
                width to (160 * density).toInt()
            }
            AttachmentKind.VIDEO -> min(width, (480 * density).toInt()).let { it to it * 9 / 16 }
            AttachmentKind.AUDIO -> min(width, (360 * density).toInt()) to (44 * density).toInt()
            else -> width to (36 * density).toInt()
        }
        val label = when (kind) {
            AttachmentKind.BROKEN -> context.getString(R.string.attachment_not_found)
            AttachmentKind.PENDING -> context.getString(R.string.attachment_waiting, file?.name.orEmpty())
            AttachmentKind.CHIP -> "${file?.name} — ${Formatter.formatShortFileSize(context, file?.size?.toLong() ?: 0)}"
            AttachmentKind.AUDIO -> file?.name.orEmpty()
            AttachmentKind.PDF -> picture?.pages?.takeIf { it > 0 }?.let { context.resources.getQuantityString(R.plurals.pages, it, it) }.orEmpty()
            else -> ""
        }
        return AttachmentSpan(kind, bitmap, label, w, h, spacing, colors, playing)
    }

    private fun picture(file: Attachment, kind: AttachmentKind, width: Int): Picture? {
        val key = "${file.id}/$width"
        cache.get(key)?.let { return it }
        if (!asked.add(key)) return null
        val path = file.localPath ?: return null
        scope.launch {
            val picture = withContext(Dispatchers.IO) { read(path, kind, width) }
            cache.put(key, picture)
            asked.remove(key)
            loaded(file.id)
        }
        return null
    }

    private fun read(path: String, kind: AttachmentKind, width: Int): Picture = when (kind) {
        AttachmentKind.IMAGE -> Picture(decodeImage(path, width), 0)
        AttachmentKind.VIDEO -> Picture(videoFrame(path), 0)
        else -> runCatching {
            ParcelFileDescriptor.open(File(path), ParcelFileDescriptor.MODE_READ_ONLY).use { descriptor ->
                PdfRenderer(descriptor).use { renderer ->
                    renderer.openPage(0).use { page ->
                        val side = min(width, (320 * context.resources.displayMetrics.density).toInt())
                        val bitmap = Bitmap.createBitmap(side, max(1, side * page.height / max(1, page.width)), Bitmap.Config.ARGB_8888)
                        bitmap.eraseColor(Color.WHITE)
                        page.render(bitmap, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                        Picture(bitmap, renderer.pageCount)
                    }
                }
            }
        }.getOrDefault(Picture(null, 0))
    }

    private fun videoFrame(path: String): Bitmap? = runCatching {
        MediaMetadataRetriever().run {
            try {
                setDataSource(path)
                frameAtTime
            } finally {
                release()
            }
        }
    }.getOrNull()
}

/** An attachment in its place in the text, drawn in the box of its line (E14). */
class AttachmentSpan(
    private val kind: AttachmentKind,
    private val bitmap: Bitmap?,
    private val label: String,
    private val boxWidth: Int,
    private val boxHeight: Int,
    private val spacing: Float,
    private val colors: EditorColors,
    private val playing: Boolean,
) : ReplacementSpan() {
    override fun getSize(paint: Paint, text: CharSequence?, start: Int, end: Int, fm: Paint.FontMetricsInt?): Int {
        if (fm != null) {
            // The line spacing of the view multiplies the height of a line; the box must come out its own height.
            val height = ceil(boxHeight / spacing).toInt()
            fm.ascent = -height
            fm.top = -height
            fm.descent = 0
            fm.bottom = 0
        }
        return boxWidth
    }

    override fun draw(canvas: Canvas, text: CharSequence?, start: Int, end: Int, x: Float, top: Int, y: Int, bottom: Int, paint: Paint) {
        val box = RectF(x, top.toFloat(), x + boxWidth, top + boxHeight.toFloat())
        val corner = boxHeight.coerceAtMost(36) / 4f + 4
        val fill = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = colors.codeBackground }
        val image = bitmap
        val shown = kind == AttachmentKind.IMAGE || kind == AttachmentKind.PDF || kind == AttachmentKind.VIDEO
        if (shown && image != null) {
            canvas.drawBitmap(image, null, box, Paint(Paint.FILTER_BITMAP_FLAG))
        } else {
            canvas.drawRoundRect(box, corner, corner, fill)
        }
        val pen = TextPaint(paint).apply {
            color = colors.text
            textSize = paint.textSize * 0.85f
            isAntiAlias = true
        }
        val baseline = box.centerY() + (pen.textSize * 0.35f)
        when (kind) {
            AttachmentKind.VIDEO -> play(canvas, box.centerX(), box.centerY(), boxHeight / 8f, false)
            AttachmentKind.AUDIO -> {
                play(canvas, x + boxHeight / 2f, box.centerY(), boxHeight / 4f, playing)
                drawLabel(canvas, label, x + boxHeight, baseline, boxWidth - boxHeight - 8f, pen)
            }
            AttachmentKind.PDF -> if (label.isNotEmpty()) {
                val width = pen.measureText(label) + 16
                val tag = RectF(box.right - width - 6, box.bottom - pen.textSize * 1.6f - 6, box.right - 6, box.bottom - 6)
                canvas.drawRoundRect(tag, 6f, 6f, Paint().apply { color = Color.argb(200, 255, 255, 255) })
                pen.color = Color.BLACK
                canvas.drawText(label, tag.left + 8, tag.bottom - pen.textSize * 0.45f, pen)
            }
            AttachmentKind.CHIP, AttachmentKind.PENDING, AttachmentKind.BROKEN -> {
                if (kind == AttachmentKind.BROKEN) pen.color = colors.broken
                drawLabel(canvas, label, x + 16, baseline, boxWidth - 32f, pen)
            }
            AttachmentKind.IMAGE -> Unit
        }
    }

    private fun drawLabel(canvas: Canvas, label: String, x: Float, baseline: Float, width: Float, pen: TextPaint) {
        val fitted = TextUtils.ellipsize(label, pen, width, TextUtils.TruncateAt.MIDDLE)
        canvas.drawText(fitted, 0, fitted.length, x, baseline, pen)
    }

    private fun play(canvas: Canvas, cx: Float, cy: Float, radius: Float, paused: Boolean) {
        val mark = Paint(Paint.ANTI_ALIAS_FLAG).apply { color = colors.accent }
        if (paused) {
            canvas.drawRect(cx - radius * 0.7f, cy - radius, cx - radius * 0.2f, cy + radius, mark)
            canvas.drawRect(cx + radius * 0.2f, cy - radius, cx + radius * 0.7f, cy + radius, mark)
        } else {
            val path = Path().apply {
                moveTo(cx - radius * 0.7f, cy - radius)
                lineTo(cx + radius, cy)
                lineTo(cx - radius * 0.7f, cy + radius)
                close()
            }
            canvas.drawPath(path, mark)
        }
    }
}
