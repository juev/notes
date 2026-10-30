package org.evsyukov.notes

import org.junit.Assert.assertEquals
import org.junit.Test
import uniffi.notes_core.Attachment

/** E30: the link an attachment gets in the text. */
class AttachmentLinkTest {
    private fun file(name: String, mime: String) = Attachment("a1", "n", name, mime, 1u, "", null)

    @Test
    fun `what shows inside is an image link`() {
        assertEquals("![фото.jpg](attachment:a1)", AttachmentLink.text(file("фото.jpg", "image/jpeg")))
        assertEquals("![клип.mov](attachment:a1)", AttachmentLink.text(file("клип.mov", "video/quicktime")))
        assertEquals("![отчёт.pdf](attachment:a1)", AttachmentLink.text(file("отчёт.pdf", "application/pdf")))
        assertEquals("[данные.csv](attachment:a1)", AttachmentLink.text(file("данные.csv", "text/csv")))
    }

    @Test
    fun `brackets in a name stay in the link`() {
        assertEquals("[a\\[1\\].txt](attachment:a1)", AttachmentLink.text(file("a[1].txt", "text/plain")))
    }
}
