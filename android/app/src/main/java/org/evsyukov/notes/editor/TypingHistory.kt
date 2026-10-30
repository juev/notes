package org.evsyukov.notes.editor

/** One change of the text by the user, with what it took out, so that it can be taken back. */
data class TypedEdit(val position: Int, val removed: String, val inserted: String) {
    /** The edit that takes this one back. */
    val inverse: TextEdit get() = TextEdit(position, inserted.length, removed)

    /** The edit itself, to apply it again. */
    val forward: TextEdit get() = TextEdit(position, removed.length, inserted)
}

/**
 * E25: the undo of what the user typed in the open note, in steps separated by pauses and by
 * formatting actions. Edits from other devices are not in it: they move the positions of what
 * is, so that undo takes back the typing and leaves them. It lives as long as the note stays open.
 */
class TypingHistory {
    private var undo = mutableListOf<MutableList<TypedEdit>>()
    private val redo = mutableListOf<List<TypedEdit>>()
    private var lastTime: Long? = null
    private var open = false

    val undoSteps: List<List<TypedEdit>> get() = undo
    val canUndo: Boolean get() = undo.isNotEmpty()
    val canRedo: Boolean get() = redo.isNotEmpty()

    /** Takes in an edit made at [time] milliseconds. True when it starts a new step. */
    fun record(edit: TypedEdit, time: Long): Boolean {
        redo.clear()
        val last = lastTime
        lastTime = time
        if (open && last != null && time - last < PAUSE_MS && undo.isNotEmpty()) {
            undo.last().add(edit)
            return false
        }
        undo.add(mutableListOf(edit))
        open = true
        return true
    }

    /** The next edit starts a new step: a formatting action stands on its own. */
    fun close() {
        open = false
    }

    /** The newest step, moved to the redo side: its edits are taken back from the last to the first. */
    fun popUndo(): List<TypedEdit>? {
        open = false
        val step = undo.removeLastOrNull() ?: return null
        redo.add(step)
        return step
    }

    /** The step undone last, moved back: its edits are applied again from the first to the last. */
    fun popRedo(): List<TypedEdit>? {
        open = false
        val step = redo.removeLastOrNull() ?: return null
        undo.add(step.toMutableList())
        return step
    }

    /**
     * An edit from another device: every position of the history moves with the text. The edit
     * is in the coordinates of the text now, the newest typed edit is one step back from them, and
     * so on. What it overlaps can no longer be taken back, nor can anything older than that.
     */
    fun outside(edit: TextEdit) {
        redo.clear()
        open = false
        var position = edit.position
        val stack = undo.map { it.toMutableList() }.toMutableList()
        for (s in stack.indices.reversed()) {
            for (e in stack[s].indices.reversed()) {
                val typed = stack[s][e]
                if (position >= typed.position + typed.inserted.length) {
                    position += typed.removed.length - typed.inserted.length
                } else if (position + edit.deleted <= typed.position) {
                    stack[s][e] = typed.copy(position = typed.position + edit.inserted.length - edit.deleted)
                } else {
                    undo = stack.drop(s + 1).toMutableList()
                    return
                }
            }
        }
        undo = stack
    }

    private companion object {
        /** A pause this long starts a new step. */
        const val PAUSE_MS = 1000L
    }
}
