package org.evsyukov.notes

import android.app.Activity
import android.content.Intent
import android.os.Bundle
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.evsyukov.notes.ui.AppModel
import uniffi.notes_core.NewNote
import uniffi.notes_core.Scope

/**
 * The debug scenarios, a step at a time: `adb shell am start -n org.evsyukov.notes/.DebugActivity --es step <step>`.
 * A step changes the data or the state of the app and brings the main screen forward, so that a picture of
 * the screen can be taken. Debug builds only: a release has no such activity.
 */
class DebugActivity : Activity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        val step = intent.getStringExtra("step").orEmpty()
        Repo.scope.launch(Repo.core) {
            run(step)
            withContext(Dispatchers.Main) {
                startActivity(Intent(this@DebugActivity, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_REORDER_TO_FRONT))
                finish()
            }
        }
    }

    private suspend fun run(step: String) {
        val name = step.substringBefore(':')
        val value = step.substringAfter(':', "")
        when (name) {
            "seed" -> seed()
            "scope" -> onMain { AppModel.select(scope(value)) }
            "open" -> onMain { AppModel.open(noteTitled(value)) }
            "close" -> onMain { AppModel.closeNote() }
        }
    }

    private suspend fun onMain(block: () -> Unit) = withContext(Dispatchers.Main) { block() }

    private fun noteTitled(title: String): String = Repo.store.notes(Scope.All).first { it.title == title }.id

    private fun scope(value: String): Scope = when {
        value == "trash" -> Scope.Trash
        value == "starred" -> Scope.Starred
        value == "archived" -> Scope.Archived
        value.startsWith("folder=") -> Scope.Folder(Repo.store.folders().first { it.name == value.substringAfter('=') }.id)
        value.startsWith("tag=") -> Scope.Tag(value.substringAfter('='))
        else -> Scope.All
    }

    private fun seed() {
        val store = Repo.store
        val work = store.createFolder("Работа", null)
        val projects = store.createFolder("Проекты", work.id)
        val diary = store.createFolder("Дневник", null)
        store.setFolderDaily(diary.id, true)
        val first = store.createNote(NewNote("Список покупок", null, listOf("дом"), "- [ ] хлеб\n- [x] молоко\n"))
        store.setPinned(first.id, true)
        val second = store.createNote(NewNote("План релиза", projects.id, listOf("работа", "релиз"), "# Этапы\n\n**Сначала** ядро, потом `macOS`.\n"))
        store.setStarred(second.id, true)
        store.createNote(NewNote("", work.id, emptyList(), "Заметка без заголовка показывается по первой строке текста"))
        store.createNote(NewNote("Запись дня", diary.id, emptyList(), "Сегодня"))
        val gone = store.createNote(NewNote("Удалённая заметка", null, emptyList(), "текст"))
        store.deleteNote(gone.id)
        Repo.changed()
    }
}
