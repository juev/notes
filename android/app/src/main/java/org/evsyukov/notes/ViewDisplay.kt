package org.evsyukov.notes

import uniffi.notes_core.FolderSort
import uniffi.notes_core.Note
import uniffi.notes_core.Scope
import uniffi.notes_core.SortBy

/** R16: how a view sorts and shows its notes. */
data class ViewDisplay(
    val sort: FolderSort = FolderSort(SortBy.EDITED, true),
    val showPreview: Boolean = true,
    val showDate: Boolean = true,
) {
    /** How the display is kept in the preferences of this device. */
    fun encoded(): String = listOf(sort.by.name, sort.descending, showPreview, showDate).joinToString(",")

    companion object {
        fun decoded(saved: String?): ViewDisplay {
            val parts = saved?.split(",")?.takeIf { it.size == 4 } ?: return ViewDisplay()
            val by = SortBy.entries.firstOrNull { it.name == parts[0] } ?: return ViewDisplay()
            return ViewDisplay(FolderSort(by, parts[1].toBoolean()), parts[2].toBoolean(), parts[3].toBoolean())
        }

        /** The key a system view keeps its display under on this device. */
        fun key(scope: Scope): String = when (scope) {
            Scope.All -> "all"
            Scope.Starred -> "starred"
            Scope.Archived -> "archived"
            Scope.Trash -> "trash"
            Scope.Untagged -> "untagged"
            Scope.NoFolder -> "noFolder"
            is Scope.Tag -> "tag"
            is Scope.Filter -> "filter:${scope.id}"
            is Scope.Folder -> "folder:${scope.id}"
        }

        /** R16, R25: the pinned first, then by the sort of the view. */
        fun sorted(notes: List<Note>, sort: FolderSort): List<Note> = notes.sortedWith { a, b ->
            if (a.pinned != b.pinned) return@sortedWith if (a.pinned) -1 else 1
            val order = when (sort.by) {
                SortBy.EDITED -> a.editedMs.compareTo(b.editedMs)
                SortBy.CREATED -> a.createdMs.compareTo(b.createdMs)
                SortBy.TITLE -> naturalOrder(a.shownTitle(), b.shownTitle())
            }
            when {
                order == 0 -> a.id.compareTo(b.id)
                sort.descending -> -order
                else -> order
            }
        }

        /** Titles as people read them: "Заметка 2" before "Заметка 10". */
        fun naturalOrder(a: String, b: String): Int {
            var i = 0
            var j = 0
            while (i < a.length && j < b.length) {
                if (a[i].isDigit() && b[j].isDigit()) {
                    val endA = (i until a.length).firstOrNull { !a[it].isDigit() } ?: a.length
                    val endB = (j until b.length).firstOrNull { !b[it].isDigit() } ?: b.length
                    val first = a.substring(i, endA).trimStart('0')
                    val second = b.substring(j, endB).trimStart('0')
                    val order = if (first.length != second.length) first.length - second.length else first.compareTo(second)
                    if (order != 0) return order
                    i = endA
                    j = endB
                } else {
                    val order = a[i].lowercaseChar().compareTo(b[j].lowercaseChar())
                    if (order != 0) return order
                    i++
                    j++
                }
            }
            return (a.length - i) - (b.length - j)
        }
    }
}

/** R3: the title a note shows: its title, its first line, or [untitled]. */
fun Note.shownTitle(untitled: String = "Untitled"): String {
    val own = title.trim()
    if (own.isNotEmpty()) return own
    val first = preview.trim()
    return if (first.isEmpty()) untitled else first.take(80)
}
