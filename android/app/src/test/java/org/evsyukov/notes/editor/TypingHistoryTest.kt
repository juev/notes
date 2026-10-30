package org.evsyukov.notes.editor

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** E25: the undo of typing in steps, with edits from other devices moving its positions. */
class TypingHistoryTest {
    private val start = 1_000_000L

    @Test
    fun `a pause and a format start a new step`() {
        val history = TypingHistory()
        assertTrue(history.record(TypedEdit(0, "", "а"), start))
        assertFalse(history.record(TypedEdit(1, "", "б"), start + 500))
        assertTrue(history.record(TypedEdit(2, "", "в"), start + 2000))
        history.close()
        assertTrue(history.record(TypedEdit(3, "", "г"), start + 2100))
        assertEquals(listOf(2, 1, 1), history.undoSteps.map { it.size })
    }

    @Test
    fun `e25 an edit before the typed text moves it`() {
        // The scenario of the specification: A types, B's edit arrives, undo takes back A's typing.
        val history = TypingHistory()
        history.record(TypedEdit(0, "", "свой текст"), start)
        history.outside(TextEdit(0, 0, "ЧУЖОЕ"))
        assertEquals(TextEdit(5, 10, ""), history.popUndo()?.first()?.inverse)
    }

    @Test
    fun `an edit after the typed text leaves it`() {
        val history = TypingHistory()
        history.record(TypedEdit(0, "", "аб"), start)
        history.record(TypedEdit(2, "", "вг"), start + 100)
        history.outside(TextEdit(6, 0, "ЧУЖОЕ"))
        assertEquals(listOf(0, 2), history.popUndo()?.map { it.position })
    }

    @Test
    fun `an edit inside the typed text ends what can be taken back`() {
        val history = TypingHistory()
        history.record(TypedEdit(0, "", "старое"), start)
        history.record(TypedEdit(6, "", "новое"), start + 5000)
        history.outside(TextEdit(2, 1, "Х"))
        assertEquals("the newest step stands, the one it overlaps is gone", 1, history.undoSteps.size)
        assertEquals("новое", history.undoSteps.first().first().inserted)
    }

    @Test
    fun `redo applies again and new typing ends it`() {
        val history = TypingHistory()
        history.record(TypedEdit(0, "x", "y"), start)
        assertNotNull(history.popUndo())
        assertEquals(TextEdit(0, 1, "y"), history.popRedo()?.first()?.forward)
        assertNotNull(history.popUndo())
        history.record(TypedEdit(0, "", "z"), start + 5000)
        assertNull(history.popRedo())
    }
}
