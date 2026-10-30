package org.evsyukov.notes.editor

import android.content.ClipboardManager
import android.content.Context
import android.graphics.Color
import android.graphics.Typeface
import android.os.Build
import android.os.SystemClock
import android.text.Editable
import android.text.InputFilter
import android.text.InputType
import android.text.Layout
import android.text.Spannable
import android.text.Spanned
import android.text.TextPaint
import android.text.TextWatcher
import android.text.method.KeyListener
import android.text.style.BackgroundColorSpan
import android.text.style.ForegroundColorSpan
import android.text.style.LeadingMarginSpan
import android.text.style.LineBreakConfigSpan
import android.text.style.LocaleSpan
import android.text.style.StrikethroughSpan
import android.text.style.UnderlineSpan
import android.util.TypedValue
import android.view.ActionMode
import android.view.Menu
import android.view.MenuItem
import android.view.MotionEvent
import android.view.ViewConfiguration
import android.view.inputmethod.BaseInputConnection
import android.widget.EditText
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.evsyukov.notes.R
import uniffi.notes_core.CodeClass
import uniffi.notes_core.MarkdownEdit
import uniffi.notes_core.MarkdownFormat
import uniffi.notes_core.MarkdownLayout
import uniffi.notes_core.TextAlign
import uniffi.notes_core.markdownFormat
import uniffi.notes_core.markdownIndent
import uniffi.notes_core.markdownLayout
import uniffi.notes_core.markdownNewline
import uniffi.notes_core.markdownPaste
import uniffi.notes_core.markdownTyped
import java.util.Locale

/** The colours of the text, from the theme of the app. */
data class EditorColors(val text: Int, val dim: Int, val link: Int, val broken: Int, val accent: Int, val codeBackground: Int, val highlight: Int, val found: Int, val dark: Boolean)

/** E40–E46, E50: what the settings of typography give the text view. */
data class Look(
    val fontBody: String,
    val fontCode: String,
    val size: Int,
    val lineHeight: Double,
    val paragraphSpacing: Double,
    val align: TextAlign,
    val hyphenate: Boolean,
    val smartTypography: Boolean,
    val spellcheck: Boolean,
    val columnCharacters: Int?,
    val colors: EditorColors,
)

/**
 * The text of a note, shown as Markdown while it is typed (E10–E16 in docs/specs/editor.md). The string
 * stays what was typed: the core says which ranges to style, hide and replace, and this view does it
 * with spans, changing only those that differ, because the cost of a keystroke grows with their number.
 */
class NoteEditText(context: Context) : EditText(context, null, 0, R.style.NoteEditor) {
    /** An edit of the text has been made, with the whole text after it. */
    var onEdit: (TextEdit, String) -> Unit = { _, _ -> }

    /** A tap on a link (E13), or on an attachment (R31, R32, R35). */
    var onOpen: (String) -> Unit = {}
    var onAttachment: (String) -> Unit = {}

    /** R52: whether a link to this note leads nowhere. */
    var linkBroken: (String) -> Boolean = { false }

    /** Files pasted into the text (R30). */
    var onUris: (List<android.net.Uri>) -> Unit = {}

    /** The text after a change of the selection, for the picker of notes (E24). */
    var onCursor: () -> Unit = {}

    /** The text has the keyboard. */
    var onFocus: (Boolean) -> Unit = {}

    /** E14: the span of an attachment in the text. */
    var attachmentSpan: (id: String, inline: Boolean, column: Int, spacing: Float) -> Any = { _, _, _, _ -> HiddenSpan() }

    /** R53: the selection goes into a new note. */
    var onNoteFromSelection: () -> Unit = {}

    /** E25: for the buttons on the bar. */
    var canUndo by mutableStateOf(false)
        private set
    var canRedo by mutableStateOf(false)
        private set

    private var history = TypingHistory()
    private var look: Look? = null
    private var textFonts = Fonts.text(context.assets, "system")
    private var codeFonts = Fonts.code(context.assets, "system")
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main.immediate)
    private var ready = false

    /** The edit `beforeTextChanged` saw, until `onTextChanged` knows what came in its place. */
    private var removed = ""

    /** An edit of the core or a replay: not new typing, and no Return of the user. */
    private var applying = false
    private var replaying = false

    /** A change from outside: another device (S40). */
    private var outside = false

    private var info: MarkdownLayout? = null
    private var infoOf = -1
    private var generation = 0
    private var parsing = false
    private var touched: List<Int> = emptyList()
    private var lastLineBlank = false
    private var painted = mutableListOf<Pair<Style, Any>>()
    private var tappable: List<Tappable> = emptyList()
    private var noSpellcheck: List<IntRange> = emptyList()
    private var windowFrom = 0
    private var windowTo = 0

    private var found: List<IntRange> = emptyList()
    private var foundSpans = mutableListOf<Any>()
    private val keys: KeyListener? = keyListener

    init {
        inputType = TEXT_INPUT
        isVerticalScrollBarEnabled = true
        // E22: Return in a list item; the core decides the edit.
        filters = arrayOf(InputFilter { source, start, end, dest, dstart, dend ->
            if (applying || outside || end - start != 1 || source[start] != '\n' || dstart != dend) return@InputFilter null
            val edit = markdownNewline(dest.toString(), dstart.toUInt()) ?: return@InputFilter null
            post { apply(edit) }
            ""
        })
        customSelectionActionModeCallback = SelectionMenu()
        addTextChangedListener(object : TextWatcher {
            override fun beforeTextChanged(s: CharSequence, start: Int, count: Int, after: Int) {
                removed = s.subSequence(start, start + count).toString()
            }

            override fun onTextChanged(s: CharSequence, start: Int, before: Int, count: Int) {
                val inserted = s.subSequence(start, start + count).toString()
                changed(start, removed, inserted)
            }

            override fun afterTextChanged(s: Editable) = Unit
        })
        ready = true
    }

    private fun changed(start: Int, removed: String, inserted: String) {
        generation++
        if (outside) return
        // A keyboard replaces the word it composes at every key: what reaches the core is the key.
        val narrow = TextEdit.between(removed, inserted)?.let { it.copy(position = it.position + start) } ?: return
        if (!replaying) {
            val typed = TypedEdit(narrow.position, removed.substring(narrow.position - start, narrow.position - start + narrow.deleted), narrow.inserted)
            history.record(typed, SystemClock.uptimeMillis())
        }
        updateHistory()
        onEdit(narrow, text.toString())
        parse()
        if (!applying && !replaying && look?.smartTypography == true && narrow.inserted.isNotEmpty()) post { smartTypography() }
    }

    /** E26: what typing replaces, as a step of its own, so that undo right after it brings back what was typed. */
    private fun smartTypography() {
        if (composing() || selectionStart != selectionEnd) return
        markdownTyped(text.toString(), selectionStart.toUInt())?.let(::apply)
    }

    private fun composing(): Boolean = BaseInputConnection.getComposingSpanStart(text) >= 0

    private fun updateHistory() {
        canUndo = history.canUndo
        canRedo = history.canRedo
    }

    /** The text of a note that was just opened, with its layout. */
    fun setNote(value: String, layout: MarkdownLayout) {
        outside = true
        setText(value)
        outside = false
        // The spans went with the text they lay on.
        painted = mutableListOf()
        history = TypingHistory()
        updateHistory()
        info = layout
        infoOf = generation
        paint()
    }

    /** An edit that comes from outside the editor: another device (S40). It keeps the selection on the same characters and is nothing the user typed. */
    fun apply(edit: TextEdit) {
        history.outside(edit)
        updateHistory()
        val start = edit.moved(selectionStart)
        val end = edit.moved(selectionEnd)
        outside = true
        text.replace(edit.position, edit.position + edit.deleted, edit.inserted)
        outside = false
        setSelection(start.coerceAtMost(length()), end.coerceAtMost(length()))
        parse()
    }

    /** What the core holds, when it differs from this text and nothing typed is on its way. */
    fun follow(core: String, pending: Boolean): Boolean {
        if (pending || composing()) return false
        apply(TextEdit.between(text.toString(), core) ?: return false)
        return true
    }

    // Undo of typing (E25)

    fun undo() {
        val step = history.popUndo() ?: return
        replay(step.reversed().map { it.inverse })
    }

    fun redo() {
        val step = history.popRedo() ?: return
        replay(step.map { it.forward })
    }

    private fun replay(edits: List<TextEdit>) {
        replaying = true
        for (edit in edits) {
            text.replace(edit.position, edit.position + edit.deleted, edit.inserted)
            setSelection((edit.position + edit.inserted.length).coerceAtMost(length()))
        }
        replaying = false
        updateHistory()
    }

    // Edits of the core (E21–E23, E26)

    /** Applies an edit of the core as one step of undo, with the selection it asks for. */
    private fun apply(edit: MarkdownEdit) {
        history.close()
        applying = true
        text.replace(edit.start.toInt(), edit.end.toInt(), edit.text)
        applying = false
        val first = minOf(edit.anchor, edit.cursor).toInt().coerceAtMost(length())
        val last = maxOf(edit.anchor, edit.cursor).toInt().coerceAtMost(length())
        setSelection(first, last)
        history.close()
    }

    /** E20, E21: a format on the selection or the word at the cursor. */
    fun format(format: MarkdownFormat) {
        if (composing()) clearComposing()
        markdownFormat(text.toString(), start.toUInt(), end.toUInt(), format)?.let(::apply)
    }

    /** E22: the level of a list item. */
    fun indent(outdent: Boolean) {
        markdownIndent(text.toString(), start.toUInt(), end.toUInt(), outdent)?.let(::apply)
    }

    private val start: Int get() = minOf(selectionStart, selectionEnd).coerceAtLeast(0)
    private val end: Int get() = maxOf(selectionStart, selectionEnd).coerceAtLeast(0)

    private fun clearComposing() = BaseInputConnection.removeComposingSpans(text)

    /** R53: the selection as offsets, for a new note made from it. */
    val selectedOffsets: Pair<Int, Int>? get() = if (end > start) start to end else null

    /** The place of `[[` before the cursor on its line, with what was typed after it. */
    val linkQuery: Pair<Int, String>?
        get() {
            if (start != end) return null
            val value = text.toString()
            val line = value.lastIndexOf('\n', start - 1) + 1
            val before = value.substring(line, start)
            val open = before.lastIndexOf("[[")
            if (open < 0) return null
            val typed = before.substring(open + 2)
            return if ("]" in typed) null else (line + open) to typed
        }

    /** The choice takes the place of `[[…`; the text of the link is the title of the moment (E31). */
    fun finishLink(title: String, id: String) {
        val query = linkQuery ?: return
        val name = title.replace("[", "\\[").replace("]", "\\]")
        val link = "[$name](note:$id)"
        history.close()
        text.replace(query.first, start, link)
        history.close()
    }

    /** E30: a link to the file at the cursor. */
    fun insertLink(link: String) {
        history.close()
        text.replace(start, end, link)
        history.close()
    }

    /** R37: scrolls to the place of a piece of the text and puts the cursor there. */
    fun reveal(piece: String) {
        val at = text.indexOf(piece)
        if (at >= 0) setSelection(at)
    }

    override fun onTextContextMenuItem(id: Int): Boolean = when (id) {
        android.R.id.paste -> paste() || super.onTextContextMenuItem(id)
        android.R.id.undo -> true.also { undo() }
        android.R.id.redo -> true.also { redo() }
        else -> super.onTextContextMenuItem(id)
    }

    /** E23: rich text comes in as Markdown of the dialect, an address over a selection as a link, a file as an attachment. */
    private fun paste(): Boolean {
        val clip = context.getSystemService(ClipboardManager::class.java)?.primaryClip ?: return false
        val item = clip.takeIf { it.itemCount > 0 }?.getItemAt(0) ?: return false
        val uri = item.uri
        if (uri != null && item.text == null && item.htmlText == null) {
            onUris((0 until clip.itemCount).mapNotNull { clip.getItemAt(it).uri })
            return true
        }
        val edit = markdownPaste(text.toString(), start.toUInt(), end.toUInt(), item.text?.toString(), item.htmlText) ?: return false
        apply(edit)
        return true
    }

    // Find (R48)

    /** The places of [query], and the one to go to first. */
    fun find(query: String): Int {
        val value = text.toString()
        val places = mutableListOf<IntRange>()
        if (query.isNotEmpty()) {
            var at = value.indexOf(query, ignoreCase = true)
            while (at >= 0) {
                places += at until at + query.length
                at = value.indexOf(query, at + query.length, ignoreCase = true)
            }
        }
        found = places
        foundSpans.forEach { text.removeSpan(it) }
        foundSpans.clear()
        val tint = look?.colors?.found ?: Color.YELLOW
        places.forEach {
            val span = BackgroundColorSpan(tint)
            text.setSpan(span, it.first, it.last + 1, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
            foundSpans += span
        }
        return places.size
    }

    /** Selects the next or the previous match from the cursor. */
    fun findStep(forward: Boolean) {
        if (found.isEmpty()) return
        val next = if (forward) found.firstOrNull { it.first >= end } ?: found.first() else found.lastOrNull { it.last + 1 <= start } ?: found.last()
        setSelection(next.first, next.last + 1)
        bringPointIntoView(next.first)
    }

    fun replaceFound(query: String, with: String, all: Boolean) {
        history.close()
        val places = if (all) found.reversed() else listOfNotNull(found.firstOrNull { it.first == start && it.last + 1 == end })
        places.forEach { text.replace(it.first, it.last + 1, with) }
        history.close()
        find(query)
        if (!all) findStep(true)
    }

    fun endFind() {
        find("")
    }

    // Styling

    /** E40–E46: the settings came in. */
    fun applyLook(new: Look) {
        if (new == look) return
        val old = look
        look = new
        textFonts = Fonts.text(context.assets, new.fontBody)
        codeFonts = Fonts.code(context.assets, new.fontCode)
        setTextSize(TypedValue.COMPLEX_UNIT_SP, new.size.toFloat())
        typeface = textFonts.regular
        setTextColor(new.colors.text)
        justificationMode = if (new.align == TextAlign.JUSTIFY) Layout.JUSTIFICATION_MODE_INTER_WORD else Layout.JUSTIFICATION_MODE_NONE
        val hyphenates = Typography.hyphenates(new.align, new.hyphenate)
        // Hyphenation works with the line breaking that looks at a whole paragraph.
        breakStrategy = if (hyphenates) Layout.BREAK_STRATEGY_HIGH_QUALITY else Layout.BREAK_STRATEGY_SIMPLE
        hyphenationFrequency = if (hyphenates) Layout.HYPHENATION_FREQUENCY_NORMAL else Layout.HYPHENATION_FREQUENCY_NONE
        if (old?.spellcheck != new.spellcheck) inputType = if (new.spellcheck) TEXT_INPUT else TEXT_INPUT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS
        place()
        painted.forEach { text.removeSpan(it.second) }
        painted.clear()
        setLineSpacing(0f, new.lineHeight.toFloat())
        paint()
    }

    /** Read-only for a locked note (R28): the text can still be selected and its links opened. */
    fun setEditable(editable: Boolean) {
        keyListener = if (editable) keys else null
        if (editable) inputType = if (look?.spellcheck == false) TEXT_INPUT or InputType.TYPE_TEXT_FLAG_NO_SUGGESTIONS else TEXT_INPUT
        isCursorVisible = editable
    }

    /** E44: the text is centred, in a column of about this many characters. */
    private fun place() {
        val horizontal = (16 * resources.displayMetrics.density).toInt()
        val characters = look?.columnCharacters
        val glyph = paint.measureText("n")
        val column = if (characters == null) 0f else glyph * characters * 1.05f
        val side = if (characters == null) horizontal else maxOf(horizontal, ((width - column) / 2).toInt())
        if (paddingLeft != side) setPadding(side, paddingTop, side, paddingBottom)
    }

    override fun onSizeChanged(w: Int, h: Int, oldw: Int, oldh: Int) {
        super.onSizeChanged(w, h, oldw, oldh)
        place()
        if (w != oldw) paint()
    }

    /** Parses off the main thread, one pass at a time; the newest text gets the next one. */
    private fun parse() {
        if (parsing) return
        parsing = true
        val value = text.toString()
        val asked = generation
        scope.launch {
            val layout = withContext(Dispatchers.Default) { runCatching { markdownLayout(value) }.getOrNull() }
            parsing = false
            if (asked == generation) {
                info = layout
                infoOf = asked
                paint()
            } else {
                parse()
            }
        }
    }

    override fun onSelectionChanged(selStart: Int, selEnd: Int) {
        super.onSelectionChanged(selStart, selEnd)
        if (!ready) return
        onCursor()
        val layout = info ?: return
        if (infoOf != generation) return
        val context = paintContext()
        // The blank line the cursor is on gets its height back, and the one it left loses it.
        val value = text
        val blank = selStart >= 0 && (selStart == 0 || value[selStart - 1] == '\n') && (selStart == value.length || value[selStart] == '\n')
        if (Painter.touched(layout, context) != touched || blank || lastLineBlank) {
            lastLineBlank = blank
            paint()
        }
    }

    override fun onFocusChanged(focused: Boolean, direction: Int, previous: android.graphics.Rect?) {
        super.onFocusChanged(focused, direction, previous)
        if (!ready) return
        onFocus(focused)
        paint()
    }

    private fun paintContext(): PaintContext {
        val measurer = TextPaint(paint)
        val hyphenates = look?.let { Typography.hyphenates(it.align, it.hyphenate) } ?: false
        return PaintContext(
            isFocused, start, end, (width - paddingLeft - paddingRight).toFloat(), hyphenates,
            paint.textSize * 1.4f, { measurer.measureText(it) }, linkBroken,
        )
    }

    /** Changes the spans to what the layout says, touching only those that differ. */
    fun paint() {
        val parsed = info ?: return
        if (infoOf != generation || composing() || look == null) return
        val result = Painter.paint(text.toString(), parsed, paintContext())
        touched = result.touched
        tappable = result.tappable
        noSpellcheck = parsed.noSpellcheck.map { it.start.toInt() until it.end.toInt() }
        val value = text
        MovedSpanFilter.install(value)
        val windowed = result.marks.size > WINDOW_MARKS
        val marks = if (windowed) inWindow(result.marks) else result.marks
        val top = layout?.takeIf { windowed && it.lineCount > 0 }?.let { it.getLineForVertical(scrollY) }
        val anchor = top?.let { layout.getLineStart(it) }
        val past = top?.let { scrollY - layout.getLineTop(it) } ?: 0
        val wanted = HashSet(marks)
        val kept = mutableListOf<Pair<Style, Any>>()
        val have = HashSet<Mark>()
        for (entry in painted) {
            val mark = Mark(entry.first, value.getSpanStart(entry.second), value.getSpanEnd(entry.second))
            if (mark in wanted && have.add(mark)) kept += entry else value.removeSpan(entry.second)
        }
        for (mark in marks) {
            if (!have.add(mark) || mark.end > value.length) continue
            span(mark)?.let {
                value.setSpan(it, mark.start, mark.end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE or spanPriority(mark.style))
                kept += mark.style to it
            }
        }
        painted = kept
        // What left the window or came into it changes the height of the text above: the screen stays on the same line.
        if (anchor != null) {
            val shown = layout ?: return
            val y = shown.getLineTop(shown.getLineForOffset(anchor.coerceAtMost(value.length))) + past
            if (y != scrollY) scrollTo(scrollX, y)
        }
    }

    /** The cost of a keystroke grows with the number of spans around it: a long note has them around the screen only. */
    private fun inWindow(marks: List<Mark>): List<Mark> {
        // Before the first layout the note shows from its start.
        val shown = layout
        val first = shown?.let { it.getLineStart(it.getLineForVertical(scrollY)) } ?: 0
        val last = shown?.let { it.getLineEnd(it.getLineForVertical(scrollY + height)) } ?: 0
        windowFrom = (first - WINDOW_REACH).coerceAtLeast(0)
        windowTo = last + WINDOW_REACH
        return marks.filter { it.end >= windowFrom && it.start <= windowTo }
    }

    override fun onScrollChanged(horiz: Int, vert: Int, oldHoriz: Int, oldVert: Int) {
        super.onScrollChanged(horiz, vert, oldHoriz, oldVert)
        if (!ready || windowTo == 0) return
        val shown = layout ?: return
        val first = shown.getLineStart(shown.getLineForVertical(vert))
        val last = shown.getLineEnd(shown.getLineForVertical(vert + height))
        val near = WINDOW_REACH / 2
        if ((windowFrom > 0 && first - windowFrom < near) || windowTo - last < near) paint()
    }

    /** What decides what wins when two spans meet: a replacement goes over whatever is under it. */
    private fun spanPriority(style: Style): Int = when (style) {
        Style.Hide, is Style.Attachment, is Style.Grid, Style.Bullet, is Style.Check, Style.Rule -> 1 shl Spannable.SPAN_PRIORITY_SHIFT
        else -> 0
    }

    private fun span(mark: Mark): Any? {
        val colors = look?.colors ?: return null
        val density = resources.displayMetrics.density
        return when (val style = mark.style) {
            Style.Hide -> HiddenSpan()
            Style.Bullet -> BulletSpan()
            is Style.Check -> CheckboxSpan(style.checked, colors.accent, colors.dim)
            Style.Rule -> RuleSpan(width - paddingLeft - paddingRight, colors.dim)
            is Style.Grid -> GridSpan(style.row, textFonts, colors.dim)
            is Style.Attachment -> attachmentSpan(style.id, style.inline, width - paddingLeft - paddingRight, lineSpacingMultiplier)
            Style.Collapse -> CollapsedSpan()
            Style.Gap -> ParagraphGapSpan(((look?.paragraphSpacing ?: 0.5) * paint.textSize).toInt())
            is Style.Heading -> HeadingSpan(HEADING_SCALES[(style.level - 1).coerceIn(0, 5)], textFonts, codeFonts)
            Style.Bold -> TraitSpan(true, false, textFonts, codeFonts)
            Style.Italic -> TraitSpan(false, true, textFonts, codeFonts)
            Style.Strike -> StrikethroughSpan()
            Style.Highlight -> BackgroundColorSpan(colors.highlight)
            Style.Code -> CodeInline(codeFonts, colors.codeBackground)
            Style.CodeFont -> CodeSpan(codeFonts)
            Style.CodeBlock -> CodeBlockSpan(colors.codeBackground)
            is Style.Token -> ForegroundColorSpan(tokenColor(style.kind, colors.dark))
            Style.Dim -> ForegroundColorSpan(colors.dim)
            is Style.Link -> LinkSpan(if (style.broken) colors.broken else colors.link)
            Style.Quote -> QuoteSpan((12 * density).toInt(), (3 * density).toInt(), colors.dim)
            is Style.Hanging -> LeadingMarginSpan.Standard(0, style.rest)
            is Style.SpaceBefore -> SpaceBeforeSpan(((look?.paragraphSpacing ?: 0.5) * 2 * paint.textSize).toInt())
            is Style.Language -> LocaleSpan(Locale.forLanguageTag(style.tag))
            Style.NoHyphen -> if (Build.VERSION.SDK_INT >= 35) LineBreakConfigSpan.createNoHyphenationSpan() else null
            Style.NoBreak -> if (Build.VERSION.SDK_INT >= 35) LineBreakConfigSpan.createNoBreakSpan() else null
        }
    }

    /** E20, R53: the formats in the menu of a selection. */
    private inner class SelectionMenu : ActionMode.Callback {
        override fun onCreateActionMode(mode: ActionMode, menu: Menu): Boolean {
            if (keyListener == null) return true
            FORMATS.forEachIndexed { index, (title, _) -> menu.add(Menu.NONE, FORMAT_ITEMS + index, FORMAT_ORDER + index, title) }
            menu.add(Menu.NONE, NEW_NOTE_ITEM, FORMAT_ORDER + FORMATS.size, R.string.new_note_from_selection)
            return true
        }

        override fun onPrepareActionMode(mode: ActionMode, menu: Menu) = false

        override fun onActionItemClicked(mode: ActionMode, item: MenuItem): Boolean {
            if (item.itemId == NEW_NOTE_ITEM) {
                onNoteFromSelection()
                mode.finish()
                return true
            }
            val format = FORMATS.getOrNull(item.itemId - FORMAT_ITEMS)?.second ?: return false
            format(format)
            return true
        }

        override fun onDestroyActionMode(mode: ActionMode) = Unit
    }

    // Taps

    private var downX = 0f
    private var downY = 0f
    private var downTime = 0L

    override fun onTouchEvent(event: MotionEvent): Boolean {
        when (event.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                downX = event.x
                downY = event.y
                downTime = event.eventTime
            }
            MotionEvent.ACTION_UP -> {
                val slop = ViewConfiguration.get(context).scaledTouchSlop
                val still = kotlin.math.abs(event.x - downX) < slop && kotlin.math.abs(event.y - downY) < slop
                val quick = event.eventTime - downTime < ViewConfiguration.getLongPressTimeout()
                val target = if (still && quick) tapped(event.x, event.y) else null
                if (target != null) {
                    // The tap was for the link: the cursor stays where it was.
                    val cancel = MotionEvent.obtain(event).apply { action = MotionEvent.ACTION_CANCEL }
                    super.onTouchEvent(cancel)
                    cancel.recycle()
                    perform(target)
                    return true
                }
            }
        }
        return super.onTouchEvent(event)
    }

    private fun tapped(x: Float, y: Float): Tap? {
        val shown = layout ?: return null
        val line = shown.getLineForVertical((y - totalPaddingTop + scrollY).toInt())
        val inX = x - totalPaddingLeft + scrollX
        if (inX > shown.getLineRight(line) + 8) return null
        var at = shown.getOffsetForHorizontal(line, inX)
        if (at > 0 && inX < shown.getPrimaryHorizontal(at)) at--
        return tappable.firstOrNull { at >= it.start && at < it.end }?.target
    }

    private fun perform(tap: Tap) {
        when (tap) {
            is Tap.Open -> onOpen(tap.url)
            is Tap.Attachment -> onAttachment(tap.id)
            is Tap.Toggle -> if (keyListener != null) {
                // The view brings the cursor into sight after an edit; the box was tapped far from it.
                val place = scrollY
                history.close()
                text.replace(tap.start, tap.start + 1, if (tap.checked) " " else "x")
                history.close()
                post { scrollTo(scrollX, place) }
            }
        }
    }

    private companion object {
        const val FORMAT_ITEMS = 1000
        const val FORMAT_ORDER = 200
        const val NEW_NOTE_ITEM = 999

        /** E20: in the order of the menu of the macOS app. */
        val FORMATS = listOf(
            R.string.bold to MarkdownFormat.Bold, R.string.italic to MarkdownFormat.Italic,
            R.string.strikethrough to MarkdownFormat.Strikethrough, R.string.code to MarkdownFormat.Code,
            R.string.highlight to MarkdownFormat.Highlight, R.string.link to MarkdownFormat.Link,
            R.string.heading1 to MarkdownFormat.Heading(1u), R.string.heading2 to MarkdownFormat.Heading(2u),
            R.string.heading3 to MarkdownFormat.Heading(3u), R.string.paragraph to MarkdownFormat.Paragraph,
            R.string.bulleted_list to MarkdownFormat.BulletList, R.string.numbered_list to MarkdownFormat.NumberedList,
            R.string.task_list to MarkdownFormat.TaskList, R.string.quote to MarkdownFormat.Quote,
        )

        /** More marks than this and only those around the screen are applied, this many characters each way. */
        const val WINDOW_MARKS = 400
        const val WINDOW_REACH = 1500

        const val TEXT_INPUT = InputType.TYPE_CLASS_TEXT or InputType.TYPE_TEXT_FLAG_MULTI_LINE or InputType.TYPE_TEXT_FLAG_CAP_SENTENCES

        /** E2, E47: levels 4 to 6 look like level 3. */
        val HEADING_SCALES = listOf(1.6f, 1.35f, 1.15f, 1.15f, 1.15f, 1.15f)

        /** E3: the colour of a token of code. */
        fun tokenColor(kind: CodeClass, dark: Boolean): Int {
            val rgb = when (kind) {
                CodeClass.KEYWORD -> if (dark) 0xFF7AB2 else 0xC2185B
                CodeClass.OPERATOR, CodeClass.MARKUP -> if (dark) 0xA0A0A0 else 0x606060
                CodeClass.STRING, CodeClass.DELETED -> if (dark) 0xFF8A80 else 0xC62828
                CodeClass.NUMBER, CodeClass.CONSTANT -> if (dark) 0xCE93D8 else 0x7B1FA2
                CodeClass.COMMENT -> if (dark) 0x8A8A8A else 0x757575
                CodeClass.TYPE -> if (dark) 0x80CBC4 else 0x00796B
                CodeClass.FUNCTION, CodeClass.TAG, CodeClass.HEADING -> if (dark) 0x82B1FF else 0x1565C0
                CodeClass.ATTRIBUTE -> if (dark) 0xFFB74D else 0xE65100
                CodeClass.PROPERTY -> if (dark) 0x9FA8DA else 0x283593
                CodeClass.INSERTED -> if (dark) 0xA5D6A7 else 0x2E7D32
            }
            return Color.rgb(rgb shr 16 and 0xFF, rgb shr 8 and 0xFF, rgb and 0xFF)
        }
    }
}

/** A code span: its font and its background (E47). */
private class CodeInline(fonts: FontSet, private val tint: Int) : android.text.style.MetricAffectingSpan() {
    private val font = CodeSpan(fonts)

    override fun updateMeasureState(paint: TextPaint) = font.updateMeasureState(paint)

    override fun updateDrawState(paint: TextPaint) {
        font.updateDrawState(paint)
        paint.bgColor = tint
    }
}

/** E13: the look of a link. A tap is told apart by the view itself. */
private class LinkSpan(private val tint: Int) : UnderlineSpan() {
    override fun updateDrawState(paint: TextPaint) {
        super.updateDrawState(paint)
        paint.color = tint
    }
}
