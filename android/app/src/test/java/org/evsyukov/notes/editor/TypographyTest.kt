package org.evsyukov.notes.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.notes_core.TextAlign

/** E41–E48: what the settings of typography come to. */
class TypographyTest {
    @Test
    fun `e41 the size stays in its range`() {
        assertEquals(16, Typography.size(null, 16))
        assertEquals(28, Typography.size(40, 16))
        assertEquals(12, Typography.size(5, 16))
    }

    @Test
    fun `the steps of the settings`() {
        assertEquals(1.2, Typography.lineHeights.first(), 1e-9)
        assertEquals(2.0, Typography.lineHeights.last(), 1e-9)
        assertEquals(9, Typography.lineHeights.size)
        assertEquals(listOf(0.0, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5), Typography.paragraphSpacings)
    }

    @Test
    fun `e45 justified text hyphenates`() {
        assertTrue(Typography.hyphenates(TextAlign.JUSTIFY, false))
        assertTrue(Typography.hyphenates(TextAlign.LEFT, true))
        assertFalse(Typography.hyphenates(TextAlign.LEFT, false))
    }

    @Test
    fun `e46 the language of a paragraph by its letters`() {
        assertEquals("ru", Typography.language("Текст на русском with a few words"))
        assertEquals("en", Typography.language("English text"))
    }

    @Test
    fun `e48 a one-letter word stays with the next word`() {
        assertFalse("not between в and лес", Typography.mayBreak("идти в лес", 7))
        assertTrue("before в is fine", Typography.mayBreak("идти в лес", 5))
        assertTrue(Typography.mayBreak("слово кот", 6))
    }

    @Test
    fun `e48 an em dash stays with the word before it`() {
        assertFalse(Typography.mayBreak("слово — другое", 6))
        assertTrue(Typography.mayBreak("слово — другое", 8))
    }

    @Test
    fun `e44 the columns of the specification`() {
        assertEquals(60, ColumnWidth.NARROW.characters)
        assertEquals(72, ColumnWidth.MEDIUM.characters)
        assertEquals(90, ColumnWidth.WIDE.characters)
        assertNull(ColumnWidth.FULL.characters)
    }
}
