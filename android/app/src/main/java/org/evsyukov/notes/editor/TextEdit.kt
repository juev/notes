package org.evsyukov.notes.editor

/** A replacement in a text, in UTF-16 units, the way `splice_utf16` of the core takes it. */
data class TextEdit(val position: Int, val deleted: Int, val inserted: String) {
    /** The text after this edit. */
    fun applied(text: String): String {
        val start = minOf(position, text.length)
        return text.replaceRange(start, minOf(start + deleted, text.length), inserted)
    }

    /** The same edit for a text that [other] has changed meanwhile (S3): both stay. */
    fun rebased(other: TextEdit): TextEdit {
        val start = other.moved(position)
        return TextEdit(start, maxOf(0, other.moved(position + deleted) - start), inserted)
    }

    /**
     * Where a position of the text before this edit is in the text after it: behind the edit it moves
     * with the text, inside what was replaced it goes to the end of the new text (S40).
     */
    fun moved(offset: Int): Int = when {
        offset <= position -> offset
        offset >= position + deleted -> offset - deleted + inserted.length
        else -> position + inserted.length
    }

    companion object {
        /**
         * The smallest edit that turns [old] into [new]: what the two do not share at their ends. Used
         * for a text that changed under the editor (S40), so that the rest of it stays in place.
         */
        fun between(old: String, new: String): TextEdit? {
            if (old == new) return null
            var start = 0
            while (start < old.length && start < new.length && old[start] == new[start]) start++
            // Not between the two halves of a surrogate pair.
            if (start > 0 && start < old.length && old[start].isLowSurrogate()) start--
            var tail = 0
            while (tail < old.length - start && tail < new.length - start && old[old.length - 1 - tail] == new[new.length - 1 - tail]) tail++
            if (tail > 0 && old[old.length - tail].isLowSurrogate()) tail--
            return TextEdit(start, old.length - start - tail, new.substring(start, new.length - tail))
        }
    }
}

/**
 * Puts edits made in a row together, so that the core opens its document once for a word and not
 * once for every key: a keystroke at the end of what is pending joins it, as does a backspace
 * over it. Any other edit starts a new one.
 */
class EditCoalescer {
    var pending: TextEdit? = null
        private set

    /** Takes in an edit; returns what must reach the core before it, when it does not join. */
    fun add(edit: TextEdit): TextEdit? {
        val current = pending
        if (current == null) {
            pending = edit
            return null
        }
        val end = current.position + current.inserted.length
        if (edit.position == end && edit.deleted == 0) {
            pending = current.copy(inserted = current.inserted + edit.inserted)
            return null
        }
        if (edit.inserted.isEmpty() && edit.position + edit.deleted == end) {
            if (edit.deleted <= current.inserted.length) {
                pending = current.copy(inserted = current.inserted.dropLast(edit.deleted))
                return null
            }
            if (current.inserted.isEmpty()) {
                pending = TextEdit(edit.position, current.deleted + edit.deleted, "")
                return null
            }
        }
        pending = edit
        return current
    }

    /** What is pending, and nothing is any longer. */
    fun take(): TextEdit? = pending.also { pending = null }
}

/** E11–E13, E16: whether the source of a span shows: while the selection touches its reveal range, its ends included. */
fun revealed(selectionStart: Int, selectionEnd: Int, start: Int, end: Int): Boolean =
    selectionStart <= end && start <= selectionEnd
