package org.evsyukov.notes.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.notes_core.markdownLayout

/** E10–E16: what the ranges of the core come to, for the real parser. */
class PainterTest {
    private fun painted(text: String, cursor: Int? = null, width: Float = 1000f): Painted {
        val at = cursor ?: 0
        val context = PaintContext(cursor != null, at, at, width, false, 10f, { it.length * 10f }, { false })
        return Painter.paint(text, markdownLayout(text), context)
    }

    private fun Painted.styled(style: Style) = marks.filter { it.style == style }.map { it.start to it.end }

    @Test
    fun `e11 markers hide outside the span and show at it`() {
        val text = "a **b** c"
        assertEquals(listOf(2 to 4, 5 to 7), painted(text, 0).styled(Style.Hide))
        assertEquals(emptyList<Pair<Int, Int>>(), painted(text, 6).styled(Style.Hide))
        assertTrue(painted(text, 6).marks.any { it.style == Style.Dim })
        assertTrue(painted(text, 0).marks.any { it.style == Style.Bold })
    }

    @Test
    fun `e12 a heading sign hides off its line and a list marker is a bullet`() {
        assertEquals(listOf(0 to 2), painted("# Заголовок\n\nтекст", 15).styled(Style.Hide))
        assertTrue(painted("# Заголовок\n\nтекст", 15).marks.any { it.style == Style.Heading(1) })
        assertEquals(listOf(0 to 1), painted("- пункт", null).styled(Style.Bullet))
    }

    @Test
    fun `e12 a checkbox stands for its brackets and a tap toggles the mark`() {
        val shown = painted("- [ ] дело", null)
        assertEquals(listOf(2 to 5), shown.styled(Style.Check(false)))
        assertEquals(listOf(Tappable(0, 5, Tap.Toggle(3, false))), shown.tappable)
    }

    @Test
    fun `e13 a link is tappable and its address hides`() {
        val text = "см. [тут](https://example.com) всё"
        val shown = painted(text, 0)
        assertEquals(Tap.Open("https://example.com"), shown.tappable.single().target)
        assertTrue(shown.styled(Style.Hide).contains(8 to 30))
    }

    @Test
    fun `e15 a table is drawn as one grid row for each of its rows`() {
        val text = "| Имя | Число |\n|-----|------:|\n| а   | 1     |\n| бб  | 22    |\n\nконец"
        val rows = painted(text, null).marks.filter { it.style is Style.Grid }
        assertEquals(3, rows.size)
        assertEquals(listOf("Имя", "а", "бб"), rows.map { (it.style as Style.Grid).row.cells.first().text })
        assertEquals(listOf(true, false, false), rows.map { (it.style as Style.Grid).row.header })
    }

    @Test
    fun `e15 the right column is aligned`() {
        val text = "| Имя | Число |\n|-----|------:|\n| а   | 1     |\n"
        val cells = painted(text, null).marks.mapNotNull { (it.style as? Style.Grid)?.row }.map { it.cells[1].x }
        assertEquals(listOf(40f, 80f), cells)
    }

    @Test
    fun `e15 a table the cursor is in stays as typed`() {
        val text = "| а | б |\n|---|---|\n| 1 | 2 |\n\nконец"
        assertFalse(painted(text, 3).marks.any { it.style is Style.Grid })
    }

    @Test
    fun `e15 a table wider than the column stays as typed`() {
        val text = "| длинная ячейка | ещё одна длинная ячейка |\n|---|---|\n| 1 | 2 |\n"
        assertFalse(painted(text, null, 100f).marks.any { it.style is Style.Grid })
    }

    @Test
    fun `e16 the fence of a code block leaves no line behind`() {
        val text = "```kotlin\nval a = 1\n```\n\nконец"
        val shown = painted(text, 28)
        assertTrue(shown.styled(Style.Collapse).contains(0 to 10))
        assertTrue(shown.marks.any { it.style is Style.Token })
    }

    @Test
    fun `e43 a blank line between blocks collapses unless the cursor is on it`() {
        val text = "один\n\nдва"
        assertEquals(listOf(4 to 5).map { (it.first + 1) to (it.second + 1) }, painted(text, 0).styled(Style.Collapse))
        assertEquals(emptyList<Pair<Int, Int>>(), painted(text, 5).styled(Style.Collapse))
    }

    @Test
    fun `e43 the space comes after a paragraph and after the last line of a block`() {
        assertEquals(listOf(0 to 5, 6 to 9), painted("один\n\nдва").styled(Style.Gap))
        val code = "```\nа\nб\n```\n"
        assertEquals("the code is one block", 1, painted(code, 12).styled(Style.Gap).count { it.first in 4..7 })
    }

    @Test
    fun `e48 a one-letter word holds on to the next word`() {
        assertEquals(listOf(5 to 8), painted("идти в лес").styled(Style.NoBreak).take(1))
    }

    @Test
    fun `e48 no span ends inside what a replacement covers`() {
        val table = "| а | б |\n|---|---|\n| в лес | 1 |\n"
        assertFalse(painted(table, null).marks.any { it.style == Style.NoBreak })
        val file = "![а б.jpg](attachment:a1)\n\nтекст"
        assertFalse(painted(file, 30).marks.any { it.style == Style.NoBreak })
    }

    @Test
    fun `e14 an attachment link becomes the attachment off its line`() {
        val text = "![фото.jpg](attachment:a1)\n\nтекст"
        assertTrue(painted(text, 30).marks.any { it.style == Style.Attachment("a1", true, 0) })
        assertFalse(painted(text, 3).marks.any { it.style is Style.Attachment })
    }
}
