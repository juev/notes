package org.evsyukov.notes.editor

import android.app.Activity
import android.content.Context
import android.graphics.Color
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.Robolectric
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.shadows.ShadowLooper
import uniffi.notes_core.TextAlign
import uniffi.notes_core.markdownLayout

/** The text view itself, on the real core: what edits do to its text and its history. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], application = android.app.Application::class)
class NoteEditTextTest {
    private fun view(text: String): NoteEditText {
        val view = NoteEditText(ApplicationProvider.getApplicationContext<Context>())
        val colors = EditorColors(Color.BLACK, Color.GRAY, Color.BLUE, Color.RED, Color.BLUE, Color.LTGRAY, Color.YELLOW, Color.YELLOW, false)
        view.applyLook(Look("system", "system", 16, 1.5, 0.5, TextAlign.LEFT, false, false, true, null, colors))
        view.setNote(text, markdownLayout(text))
        return view
    }

    @Test
    fun `e25 undo brings back a line end inside a list as it was`() {
        val view = view("- a\n- b")
        view.setSelection(7)
        val text = view.text
        repeat(4) { text.delete(text.length - 1, text.length) }
        assertEquals("- a", text.toString())
        view.undo()
        assertEquals("- a\n- b", view.text.toString())
    }

    @Test
    fun `e22 return in a list goes on with the list`() {
        val view = view("- a")
        // A view runs what is posted to it only in a window.
        Robolectric.buildActivity(Activity::class.java).setup().get().setContentView(view)
        view.setSelection(3)
        view.text.insert(3, "\n")
        ShadowLooper.idleMainLooper()
        assertEquals("- a\n- ", view.text.toString())
    }

    @Test
    fun `r48 replacing reads the places anew after the text moved`() {
        val view = view("один два один")
        view.find("один")
        view.text.delete(0, 5)
        view.replaceFound("один", "три", true)
        assertEquals("два три", view.text.toString())
    }

    @Test
    fun `s40 an edit from elsewhere is not undone`() {
        val view = view("текст")
        view.setSelection(5)
        view.text.insert(5, "!")
        view.apply(TextEdit(0, 0, "ЧУЖОЕ "))
        view.undo()
        assertEquals("ЧУЖОЕ текст", view.text.toString())
        assertFalse(view.canUndo)
    }
}
