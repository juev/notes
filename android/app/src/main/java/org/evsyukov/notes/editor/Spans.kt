package org.evsyukov.notes.editor

import android.graphics.Canvas
import android.graphics.Color
import android.graphics.Paint
import android.graphics.Typeface
import android.text.Editable
import android.text.Layout
import android.text.SpanWatcher
import android.text.Spannable
import android.text.Spanned
import android.text.TextPaint
import android.text.TextWatcher
import android.text.style.CharacterStyle
import android.text.style.LeadingMarginSpan
import android.text.style.LineBackgroundSpan
import android.text.style.LineHeightSpan
import android.text.style.MetricAffectingSpan
import android.text.style.ParagraphStyle
import android.text.style.ReplacementSpan
import android.text.style.SuggestionSpan
import kotlin.math.ceil

/**
 * Stands between the text and those who watch its spans. An edit moves every span behind it, and the
 * layout measures a paragraph anew for each one that affects the layout, though the edit has told it
 * everything already. A style that only moved is not passed on; the selection, the composing text and
 * suggestions are. Reached through public interfaces; a watcher it does not recognise works as it did.
 */
class MovedSpanFilter private constructor(private val inner: SpanWatcher) :
    TextWatcher,
    SpanWatcher {
    private val text = inner as? TextWatcher

    override fun beforeTextChanged(s: CharSequence, start: Int, count: Int, after: Int) {
        text?.beforeTextChanged(s, start, count, after)
    }

    override fun onTextChanged(s: CharSequence, start: Int, before: Int, count: Int) {
        text?.onTextChanged(s, start, before, count)
    }

    override fun afterTextChanged(s: Editable) {
        text?.afterTextChanged(s)
    }

    override fun onSpanAdded(s: Spannable, what: Any, start: Int, end: Int) = inner.onSpanAdded(s, what, start, end)

    override fun onSpanRemoved(s: Spannable, what: Any, start: Int, end: Int) = inner.onSpanRemoved(s, what, start, end)

    override fun onSpanChanged(s: Spannable, what: Any, ostart: Int, oend: Int, nstart: Int, nend: Int) {
        val style = (what is CharacterStyle || what is ParagraphStyle) && what !is SuggestionSpan
        if (style && nstart != ostart && nend - nstart == oend - ostart) return
        inner.onSpanChanged(s, what, ostart, oend, nstart, nend)
    }

    companion object {
        private const val LAYOUT = "android.text.DynamicLayout"

        /** Puts a filter before the watcher of the layout; a layout made anew brings a new one. */
        fun install(text: Spannable) {
            val watcher = text.getSpans(0, text.length, SpanWatcher::class.java)
                .firstOrNull { it !is MovedSpanFilter && it.javaClass.name.startsWith(LAYOUT) } ?: return
            text.getSpans(0, text.length, MovedSpanFilter::class.java).forEach(text::removeSpan)
            val flags = text.getSpanFlags(watcher)
            text.removeSpan(watcher)
            text.setSpan(MovedSpanFilter(watcher), 0, text.length, flags)
        }
    }
}

/** Markup that takes no room. */
class HiddenSpan : ReplacementSpan() {
    override fun getSize(paint: Paint, text: CharSequence?, start: Int, end: Int, fm: Paint.FontMetricsInt?) = 0

    override fun draw(canvas: Canvas, text: CharSequence?, start: Int, end: Int, x: Float, top: Int, y: Int, bottom: Int, paint: Paint) = Unit
}

/** E12: the bullet that stands for a list marker. */
class BulletSpan : ReplacementSpan() {
    override fun getSize(paint: Paint, text: CharSequence?, start: Int, end: Int, fm: Paint.FontMetricsInt?) = ceil(paint.measureText("•")).toInt()

    override fun draw(canvas: Canvas, text: CharSequence?, start: Int, end: Int, x: Float, top: Int, y: Int, bottom: Int, paint: Paint) {
        canvas.drawText("•", x, y.toFloat(), paint)
    }
}

/** E12: the checkbox that stands for `[ ]` or `[x]`. */
class CheckboxSpan(private val checked: Boolean, private val on: Int, private val off: Int) : ReplacementSpan() {
    override fun getSize(paint: Paint, text: CharSequence?, start: Int, end: Int, fm: Paint.FontMetricsInt?) = ceil(paint.textSize * 1.1f).toInt()

    override fun draw(canvas: Canvas, text: CharSequence?, start: Int, end: Int, x: Float, top: Int, y: Int, bottom: Int, paint: Paint) {
        val side = paint.textSize * 0.95f
        val low = y + paint.descent() * 0.3f
        val box = android.graphics.RectF(x, low - side, x + side, low)
        val pen = Paint(Paint.ANTI_ALIAS_FLAG).apply {
            color = if (checked) on else off
            style = if (checked) Paint.Style.FILL else Paint.Style.STROKE
            strokeWidth = side * 0.09f
        }
        canvas.drawRoundRect(box, side * 0.18f, side * 0.18f, pen)
        if (checked) {
            pen.color = Color.WHITE
            pen.style = Paint.Style.STROKE
            pen.strokeWidth = side * 0.13f
            pen.strokeCap = Paint.Cap.ROUND
            canvas.drawLine(x + side * 0.22f, low - side * 0.48f, x + side * 0.42f, low - side * 0.27f, pen)
            canvas.drawLine(x + side * 0.42f, low - side * 0.27f, x + side * 0.78f, low - side * 0.72f, pen)
        }
    }
}

/** E12: a thematic break as a line across the column. */
class RuleSpan(private val width: Int, private val tint: Int) : ReplacementSpan() {
    override fun getSize(paint: Paint, text: CharSequence?, start: Int, end: Int, fm: Paint.FontMetricsInt?) = width

    override fun draw(canvas: Canvas, text: CharSequence?, start: Int, end: Int, x: Float, top: Int, y: Int, bottom: Int, paint: Paint) {
        val pen = Paint().apply { color = tint }
        val middle = (top + bottom) / 2f
        canvas.drawRect(x, middle, x + width, middle + 2, pen)
    }
}

/** A line of a table drawn as a grid (E15). */
class GridSpan(private val row: GridRow, private val fonts: FontSet, private val tint: Int) : ReplacementSpan() {
    override fun getSize(paint: Paint, text: CharSequence?, start: Int, end: Int, fm: Paint.FontMetricsInt?) = ceil(row.width).toInt()

    override fun draw(canvas: Canvas, text: CharSequence?, start: Int, end: Int, x: Float, top: Int, y: Int, bottom: Int, paint: Paint) {
        val pen = TextPaint(paint).apply { typeface = fonts.pick(row.header, false) }
        row.cells.forEach { canvas.drawText(it.text, x + it.x, y.toFloat(), pen) }
        if (row.header) {
            val line = Paint().apply { color = tint }
            canvas.drawRect(x, bottom - 2f, x + row.width, bottom - 1f, line)
        }
    }
}

/** A line that takes no height: a hidden fence, the blank line between two blocks (E16, E43). */
class CollapsedSpan : LineHeightSpan {
    override fun chooseHeight(text: CharSequence, start: Int, end: Int, spanstartv: Int, lineHeight: Int, fm: Paint.FontMetricsInt) {
        fm.top = 0
        fm.ascent = 0
        fm.descent = 0
        fm.bottom = 0
    }
}

/**
 * E43: the space after a paragraph, on its last line. The height of a line is the line spacing of
 * the view, which multiplies this space too, so it comes here divided. A span of its own on each
 * paragraph: one over the whole text would grow with every key, and the view lays out all that a
 * changed span covers.
 */
class ParagraphGapSpan(private val gap: Int) : LineHeightSpan {
    override fun chooseHeight(text: CharSequence, start: Int, end: Int, spanstartv: Int, lineHeight: Int, fm: Paint.FontMetricsInt) {
        if (text !is Spanned || end != text.getSpanEnd(this) || fm.descent <= fm.ascent) return
        fm.descent += gap
        fm.bottom += gap
    }
}

/** E47: the space before a heading. */
class SpaceBeforeSpan(private val space: Int) : LineHeightSpan {
    override fun chooseHeight(text: CharSequence, start: Int, end: Int, spanstartv: Int, lineHeight: Int, fm: Paint.FontMetricsInt) {
        if (start == 0 || text[start - 1] != '\n') return
        fm.ascent -= space
        fm.top -= space
    }
}

/** E2, E47: the size and the weight of a heading. */
class HeadingSpan(private val scale: Float, private val text: FontSet, private val code: FontSet) : MetricAffectingSpan() {
    private fun apply(paint: TextPaint) {
        paint.textSize *= scale
        paint.typeface = traited(paint.typeface, text, code, bold = true, italic = false)
    }

    override fun updateMeasureState(paint: TextPaint) = apply(paint)

    override fun updateDrawState(paint: TextPaint) = apply(paint)
}

/** Bold or italic in the font the text around it has. */
class TraitSpan(private val bold: Boolean, private val italic: Boolean, private val text: FontSet, private val code: FontSet) : MetricAffectingSpan() {
    private fun apply(paint: TextPaint) {
        paint.typeface = traited(paint.typeface, text, code, bold, italic)
    }

    override fun updateMeasureState(paint: TextPaint) = apply(paint)

    override fun updateDrawState(paint: TextPaint) = apply(paint)
}

private fun traited(face: Typeface?, text: FontSet, code: FontSet, bold: Boolean, italic: Boolean): Typeface {
    val set = if (code.holds(face)) code else text
    return set.pick(bold || face?.isBold == true, italic || face?.isItalic == true)
}

/** E47: code is 0.9 of the text size, in the code font. */
class CodeSpan(private val code: FontSet) : MetricAffectingSpan() {
    private fun apply(paint: TextPaint) {
        paint.textSize *= 0.9f
        val face = paint.typeface
        paint.typeface = code.pick(face?.isBold == true, face?.isItalic == true)
    }

    override fun updateMeasureState(paint: TextPaint) = apply(paint)

    override fun updateDrawState(paint: TextPaint) = apply(paint)
}

/** E16: the background of a code block across the column. */
class CodeBlockSpan(private val tint: Int) : LineBackgroundSpan {
    override fun drawBackground(canvas: Canvas, paint: Paint, left: Int, right: Int, top: Int, baseline: Int, bottom: Int, text: CharSequence, start: Int, end: Int, lineNumber: Int) {
        val color = paint.color
        paint.color = tint
        canvas.drawRect(left.toFloat(), top.toFloat(), right.toFloat(), bottom.toFloat(), paint)
        paint.color = color
    }
}

/** E47: a quote is indented, with a line on the left. */
class QuoteSpan(private val indent: Int, private val bar: Int, private val tint: Int) : LeadingMarginSpan {
    override fun getLeadingMargin(first: Boolean) = indent

    override fun drawLeadingMargin(canvas: Canvas, paint: Paint, x: Int, dir: Int, top: Int, baseline: Int, bottom: Int, text: CharSequence, start: Int, end: Int, first: Boolean, layout: Layout) {
        val color = paint.color
        val style = paint.style
        paint.color = tint
        paint.style = Paint.Style.FILL
        canvas.drawRect(x.toFloat(), top.toFloat(), (x + dir * bar).toFloat(), bottom.toFloat(), paint)
        paint.color = color
        paint.style = style
    }
}
