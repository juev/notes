package org.evsyukov.notes

import uniffi.notes_core.Attachment

/** E30: an attachment in the text: an image link for what shows inside the note, a link otherwise. */
object AttachmentLink {
    fun isInline(mime: String): Boolean =
        mime.startsWith("image/") || mime.startsWith("video/") || mime.startsWith("audio/") || mime == "application/pdf"

    fun text(file: Attachment): String {
        val name = file.name.replace("[", "\\[").replace("]", "\\]")
        return "${if (isInline(file.mime)) "!" else ""}[$name](attachment:${file.id})"
    }
}
