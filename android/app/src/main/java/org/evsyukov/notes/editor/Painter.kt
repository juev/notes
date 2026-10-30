package org.evsyukov.notes.editor

import uniffi.notes_core.CodeClass
import uniffi.notes_core.MarkdownAlign
import uniffi.notes_core.MarkdownKind
import uniffi.notes_core.MarkdownLayout

/** A cell of a table as it is drawn: its text and where it starts. */
data class GridCell(val text: String, val x: Float)

/** A line of a table as it is drawn. */
data class GridRow(val cells: List<GridCell>, val width: Float, val header: Boolean)

/** What a range of the text looks like. A mark says what to show; the view turns it into a span. */
sealed interface Style {
    data object Hide : Style
    data object Bullet : Style
    data class Check(val checked: Boolean) : Style
    data object Rule : Style
    data class Grid(val row: GridRow) : Style
    data class Attachment(val id: String, val inline: Boolean, val version: Int) : Style
    data object Collapse : Style
    data object Gap : Style
    data class Heading(val level: Int) : Style
    data object Bold : Style
    data object Italic : Style
    data object Strike : Style
    data object Highlight : Style
    data object Code : Style
    data object CodeFont : Style
    data object CodeBlock : Style
    data class Token(val kind: CodeClass) : Style
    data object Dim : Style
    data class Link(val broken: Boolean) : Style
    data object Quote : Style
    data class Hanging(val rest: Int) : Style
    data class SpaceBefore(val lines: Boolean) : Style
    data class Language(val tag: String) : Style
    data object NoHyphen : Style
    data object NoBreak : Style
}

data class Mark(val style: Style, val start: Int, val end: Int)

/** What can be tapped in the text. */
data class Tappable(val start: Int, val end: Int, val target: Tap)

sealed interface Tap {
    data class Open(val url: String) : Tap
    data class Toggle(val start: Int, val checked: Boolean) : Tap
    data class Attachment(val id: String) : Tap
}

/** The whole look of a text for one selection. */
class Painted(val marks: List<Mark>, val tappable: List<Tappable>, val touched: List<Int>)

/** E41–E46 and the way a line of text is measured. */
class PaintContext(
    val focused: Boolean,
    val selectionStart: Int,
    val selectionEnd: Int,
    val columnWidth: Float,
    val hyphenates: Boolean,
    val gap: Float,
    val measure: (String) -> Float,
    val linkBroken: (String) -> Boolean,
    val attachmentVersion: (String) -> Int = { 0 },
)

/**
 * Turns the ranges of the core into marks, after `restyle` of the macOS text view (E10–E16). It keeps no
 * state and draws nothing, so what a text looks like can be tested without a screen.
 */
object Painter {
    private fun lineStart(text: String, offset: Int): Int = text.lastIndexOf('\n', offset - 1) + 1

    private fun lineEnd(text: String, offset: Int): Int = text.indexOf('\n', offset).let { if (it < 0) text.length else it }

    /** The reveal ranges the selection touches, by number. None without the keyboard. */
    fun touched(info: MarkdownLayout, context: PaintContext): List<Int> {
        if (!context.focused) return emptyList()
        return info.spans.indices.filter {
            revealed(context.selectionStart, context.selectionEnd, info.spans[it].revealStart.toInt(), info.spans[it].revealEnd.toInt())
        }
    }

    fun paint(text: String, info: MarkdownLayout, context: PaintContext): Painted {
        val marks = mutableListOf<Mark>()
        val tappable = mutableListOf<Tappable>()
        val shows = { start: Int, end: Int -> context.focused && revealed(context.selectionStart, context.selectionEnd, start, end) }
        val length = text.length

        fun mark(style: Style, start: Int, end: Int) {
            if (end > start) marks += Mark(style, start, end)
        }

        /** Spans that reach over a line end stop at it: the layout breaks a paragraph there anyway. */
        fun hide(start: Int, end: Int) {
            var from = start
            while (from < end) {
                val stop = minOf(lineEnd(text, from), end)
                mark(Style.Hide, from, stop)
                from = stop + 1
            }
        }

        /** Markup that is a whole line, a code fence, leaves no empty line behind when hidden. */
        fun collapse(start: Int, end: Int) {
            val first = lineStart(text, start)
            val last = lineEnd(text, start)
            if (first == start && last == end && (last < length || first > 0)) mark(Style.Collapse, first, minOf(last + 1, length))
        }

        // E15: the tables to draw as a grid. A table stays as typed while the cursor is in it, and where
        // it does not start its line: in a list item or a quote.
        val grids = info.tables.mapNotNull { table ->
            val start = table.start.toInt()
            val end = table.end.toInt()
            if (end > length || shows(start, end) || lineStart(text, start) != start) return@mapNotNull null
            val lines = mutableListOf<IntRange>()
            var from = start
            while (from <= end) {
                val stop = minOf(lineEnd(text, from), end)
                if (stop > from) lines += from until stop
                from = stop + 1
            }
            if (lines.size == table.rows.size + 1) Triple(table, lines, start until end) else null
        }
        val inGrid = { start: Int, end: Int -> grids.any { start <= it.third.last && end > it.third.first } }

        // E43: the blank line between two blocks takes no room of its own, unless the cursor is on it.
        var line = 0
        while (line < length) {
            val stop = lineEnd(text, line)
            val onCursor = context.focused && context.selectionStart in line..stop
            if (stop == line && stop + 1 < length && !onCursor) mark(Style.Collapse, line, line + 1)
            // E46: hyphenation follows the language of each paragraph.
            if (context.hyphenates && stop > line) mark(Style.Language(Typography.language(text.substring(line, stop))), line, stop)
            line = stop + 1
        }

        val seenHidden = mutableListOf<Mark>()
        for (span in info.spans) {
            val start = span.start.toInt()
            val end = span.end.toInt()
            if (end > length || span.block.toInt() >= info.blocks.size) continue
            val shown = shows(span.revealStart.toInt(), span.revealEnd.toInt())
            when (val kind = span.kind) {
                is MarkdownKind.Heading -> {
                    mark(Style.Heading(kind.level.toInt()), start, end)
                    mark(Style.SpaceBefore(true), lineStart(text, start), lineEnd(text, start))
                    mark(Style.NoHyphen, start, end)
                }
                MarkdownKind.Strong -> mark(Style.Bold, start, end)
                MarkdownKind.Emphasis -> mark(Style.Italic, start, end)
                MarkdownKind.Strikethrough -> mark(Style.Strike, start, end)
                MarkdownKind.Highlight -> mark(Style.Highlight, start, end)
                MarkdownKind.Code -> mark(Style.Code, start, end)
                MarkdownKind.CodeBlock -> {
                    mark(Style.CodeFont, start, end)
                    mark(Style.CodeBlock, start, end)
                    mark(Style.NoHyphen, start, end)
                }
                is MarkdownKind.Token -> mark(Style.Token(kind.`class`), start, end)
                MarkdownKind.TableRow -> {
                    if (!inGrid(start, end)) mark(Style.CodeFont, start, end)
                    mark(Style.NoHyphen, start, end)
                }
                MarkdownKind.Quote -> {
                    var from = lineStart(text, start)
                    while (from < end) {
                        val stop = lineEnd(text, from)
                        mark(Style.Quote, from, minOf(stop + 1, length))
                        from = stop + 1
                    }
                }
                is MarkdownKind.Link -> {
                    tappable += Tappable(start, end, Tap.Open(kind.url))
                    mark(Style.Link(false), start, end)
                }
                is MarkdownKind.NoteLink -> {
                    tappable += Tappable(start, end, Tap.Open("note:${kind.id}"))
                    // R52: a link to a note in the Trash or deleted for good shows as broken.
                    mark(Style.Link(context.linkBroken(kind.id)), start, end)
                }
                is MarkdownKind.AttachmentLink -> {
                    // E14: the line of the link is source while the cursor is on it.
                    if (shown) {
                        mark(Style.Dim, start, end)
                    } else {
                        tappable += Tappable(start, end, Tap.Attachment(kind.id))
                        mark(Style.Attachment(kind.id, kind.inline, context.attachmentVersion(kind.id)), start, end)
                    }
                }
                is MarkdownKind.ListMarker -> {
                    if (shown && !kind.ordered) mark(Style.Dim, start, end)
                    if (!kind.ordered && !shown && end - start == 1) mark(Style.Bullet, start, end)
                    // Wrapped lines of an item start under its text, not under its marker.
                    val first = lineStart(text, start)
                    mark(Style.Hanging(context.measure(text.substring(first, end) + " ").toInt()), first, minOf(lineEnd(text, start) + 1, length))
                }
                MarkdownKind.QuoteMarker -> if (shown) mark(Style.Dim, start, end) else hide(start, end)
                is MarkdownKind.Checkbox -> {
                    // The brackets come back only when the cursor is right at them.
                    val touched = context.focused && context.selectionStart <= end && start <= context.selectionEnd
                    if (touched || end - start != 3) {
                        mark(Style.Dim, start, end)
                    } else {
                        // The marker before it is hidden and has no width: a tap on the box lands at the start of the line.
                        tappable += Tappable(lineStart(text, start), end, Tap.Toggle(start + 1, kind.checked))
                        mark(Style.Check(kind.checked), start, end)
                    }
                }
                MarkdownKind.Rule -> if (shown) mark(Style.Dim, start, end) else mark(Style.Rule, start, end)
                MarkdownKind.Markup -> {
                    if (shown) {
                        mark(Style.Dim, start, end)
                    } else {
                        hide(start, end)
                        seenHidden += Mark(Style.Hide, start, end)
                        collapse(start, end)
                    }
                }
            }
        }

        // Nothing inside a grid shows but the grid: it is one span over each of its lines.
        for ((table, lines, range) in grids) {
            val grid = grid(text, table, lines, seenHidden, context)
            if (grid == null) {
                lines.forEach { mark(Style.CodeFont, it.first, it.last + 1) }
                continue
            }
            marks.removeAll { it.start >= range.first && it.end <= range.last + 1 && it.style != Style.NoHyphen }
            val rule = lines[1]
            mark(Style.Hide, rule.first, rule.last + 1)
            mark(Style.Collapse, rule.first, minOf(rule.last + 2, length))
            (listOf(lines[0]) + lines.drop(2)).zip(grid).forEach { (row, shape) -> mark(Style.Grid(shape), row.first, row.last + 1) }
        }

        // E48: a one-letter Russian word stays with the next word, an em dash with the word before it.
        var space = text.indexOf(' ')
        while (space in 0 until length - 1) {
            val next = space + 1
            if (!Typography.mayBreak(text, next)) mark(Style.NoBreak, maxOf(0, space - 1), minOf(next + 1, length))
            space = text.indexOf(' ', next)
        }
        // E43: the space after a paragraph. A code block, a table and a quote are one block: it comes after
        // their last line.
        val blocks = marks.filter { it.style == Style.CodeBlock || it.style is Style.Grid || it.style == Style.Quote }
        val inBlock = { offset: Int -> blocks.any { offset >= it.start && offset < it.end } }
        val collapsed = marks.filter { it.style == Style.Collapse }.map { it.start }.toHashSet()
        var paragraph = 0
        while (paragraph < length) {
            val stop = lineEnd(text, paragraph)
            val next = stop + 1
            // A line that takes no height, a fence or the dashes of a table, is passed over.
            val after = if (next in collapsed) lineEnd(text, next) + 1 else next
            val last = after >= length || !inBlock(after)
            if (stop > paragraph && paragraph !in collapsed && (!inBlock(paragraph) || last)) mark(Style.Gap, paragraph, minOf(next, length))
            paragraph = next
        }

        // A span that ends inside a replacement cuts it in two, and each part draws the whole of it.
        val replaced = marks.filter { it.style is Style.Grid || it.style is Style.Attachment || it.style == Style.Rule }
        marks.removeAll { mark ->
            mark.style == Style.NoBreak && replaced.any { mark.start < it.end && mark.end > it.start }
        }
        return Painted(marks, tappable, touched(info, context))
    }

    /** The rows of a table with the place of each cell; null when it does not fit the column or its cells do not lie in order. */
    private fun grid(text: String, table: uniffi.notes_core.MarkdownTable, lines: List<IntRange>, hidden: List<Mark>, context: PaintContext): List<GridRow>? {
        val columns = table.columns.size
        val rows = listOf(lines[0]) + lines.drop(2)
        if (columns == 0 || table.rows.any { it.cells.size != columns }) return null
        val seen = table.rows.mapIndexed { index, row ->
            row.cells.map { cell ->
                val start = cell.start.toInt()
                val end = cell.end.toInt()
                if (start < rows[index].first || end > rows[index].last + 1) return null
                seenText(text, start, end, hidden)
            }
        }
        val widths = seen.map { row -> row.map(context.measure) }
        val column = (0 until columns).map { index -> widths.maxOf { it[index] } }
        val total = column.sum() + context.gap * (columns - 1)
        // A grid is not wrapped: a table wider than the note stays as typed.
        if (context.columnWidth > 0 && total > context.columnWidth) return null
        return table.rows.mapIndexed { index, row ->
            var left = 0f
            val cells = seen[index].mapIndexed { number, cell ->
                val spare = column[number] - widths[index][number]
                val x = left + when (table.columns[number]) {
                    MarkdownAlign.RIGHT -> spare
                    MarkdownAlign.CENTER -> spare / 2
                    else -> 0f
                }
                left += column[number] + context.gap
                GridCell(cell, x)
            }
            GridRow(cells, total, row.header)
        }
    }

    /** A cell as it is seen: without the markup hidden in it. */
    private fun seenText(text: String, start: Int, end: Int, hidden: List<Mark>): String {
        val out = StringBuilder()
        for (index in start until end) {
            if (hidden.none { index >= it.start && index < it.end }) out.append(text[index])
        }
        return out.toString()
    }
}
