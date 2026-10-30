package org.evsyukov.notes

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.notes_core.FolderSort
import uniffi.notes_core.Note
import uniffi.notes_core.SortBy

/** R3, R16, R25: what the list of notes shows and in what order. */
class ViewDisplayTest {
    private fun note(id: String, title: String = "", preview: String = "", pinned: Boolean = false, edited: ULong = 0u) =
        Note(id, title, null, emptyList(), pinned, false, false, false, false, 0u, edited, null, 0u, preview)

    @Test
    fun `r3 the title falls back to the first line and then to untitled`() {
        assertEquals("Заголовок", note("a", title = "Заголовок", preview = "текст").shownTitle())
        assertEquals("первая строка", note("a", title = "  ", preview = "первая строка").shownTitle())
        assertEquals("Untitled", note("a").shownTitle())
    }

    @Test
    fun `r25 pinned notes come first under any sort`() {
        val notes = listOf(note("old", edited = 1u), note("new", edited = 3u), note("pinned", pinned = true, edited = 2u))
        assertEquals(listOf("pinned", "new", "old"), ViewDisplay.sorted(notes, FolderSort(SortBy.EDITED, true)).map { it.id })
        assertEquals(listOf("pinned", "old", "new"), ViewDisplay.sorted(notes, FolderSort(SortBy.EDITED, false)).map { it.id })
    }

    @Test
    fun `titles sort as people read them`() {
        val notes = listOf(note("b", title = "Заметка 10"), note("a", title = "Заметка 2"))
        assertEquals(listOf("a", "b"), ViewDisplay.sorted(notes, FolderSort(SortBy.TITLE, false)).map { it.id })
    }

    @Test
    fun `r16 the display survives its stored form`() {
        val display = ViewDisplay(FolderSort(SortBy.CREATED, false), showPreview = false, showDate = true)
        assertEquals(display, ViewDisplay.decoded(display.encoded()))
        assertEquals(ViewDisplay(), ViewDisplay.decoded("nonsense"))
        assertEquals(ViewDisplay(), ViewDisplay.decoded(null))
    }
}
