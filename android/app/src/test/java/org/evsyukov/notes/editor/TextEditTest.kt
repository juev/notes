package org.evsyukov.notes.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** R4, S40: what typing sends to the core, and what comes in from another device. */
class TextEditTest {
    @Test
    fun `keystrokes in a row join one edit`() {
        val edits = EditCoalescer()
        assertNull(edits.add(TextEdit(5, 0, "с")))
        assertNull(edits.add(TextEdit(6, 0, "л")))
        assertNull(edits.add(TextEdit(7, 0, "😀")))
        assertNull("backspace over the emoji", edits.add(TextEdit(7, 2, "")))
        assertEquals(TextEdit(5, 0, "сл"), edits.take())
        assertNull(edits.take())
    }

    @Test
    fun `an edit elsewhere sends what waits`() {
        val edits = EditCoalescer()
        assertNull(edits.add(TextEdit(5, 0, "а")))
        assertEquals(TextEdit(5, 0, "а"), edits.add(TextEdit(0, 0, "б")))
        assertEquals(TextEdit(0, 0, "б"), edits.take())
    }

    @Test
    fun `backspaces in a row join one deletion`() {
        val edits = EditCoalescer()
        assertNull(edits.add(TextEdit(4, 1, "")))
        assertNull(edits.add(TextEdit(3, 1, "")))
        assertEquals(TextEdit(3, 2, ""), edits.take())
    }

    @Test
    fun `the difference leaves what the texts share`() {
        assertNull(TextEdit.between("а", "а"))
        assertEquals(TextEdit(7, 0, "новое "), TextEdit.between("начало конец", "начало новое конец"))
        assertEquals(TextEdit(1, 1, ""), TextEdit.between("abc", "ac"))
        // Two emoji that share a first half are not cut between their halves.
        assertEquals(TextEdit(0, 2, "😁"), TextEdit.between("😀", "😁"))
    }

    @Test
    fun `positions move with the text`() {
        val edit = TextEdit(5, 2, "новое")
        assertEquals(3, edit.moved(3))
        assertEquals(5, edit.moved(5))
        assertEquals("inside what was replaced: to the end of the new text", 10, edit.moved(6))
        assertEquals(12, edit.moved(9))
    }

    @Test
    fun `an edit gives the text after it`() {
        assertEquals("а😀в", TextEdit(1, 1, "😀").applied("абв"))
    }

    @Test
    fun `s3 an edit moves with the text before it`() {
        assertEquals(TextEdit(18, 0, "X"), TextEdit(11, 0, "X").rebased(TextEdit(0, 0, "REMOTE-")))
    }

    @Test
    fun `s3 an edit before the other stays where it is`() {
        val typed = TextEdit(1, 1, "X")
        assertEquals(typed, typed.rebased(TextEdit(5, 0, "Y")))
    }

    @Test
    fun `s3 a deletion shrinks when the other took part of it`() {
        assertEquals(TextEdit(2, 2, ""), TextEdit(2, 4, "").rebased(TextEdit(3, 2, "")))
    }

    @Test
    fun `e11 the ends of the reveal range count`() {
        // `a **b** c`: the markers of the bold span reveal over 2...7.
        assertFalse(revealed(1, 1, 2, 7))
        assertTrue(revealed(2, 2, 2, 7))
        assertTrue(revealed(7, 7, 2, 7))
        assertFalse(revealed(8, 8, 2, 7))
        assertTrue(revealed(0, 3, 2, 7))
    }
}
