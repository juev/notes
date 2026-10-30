package org.evsyukov.notes

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.evsyukov.notes.ui.AppModel
import uniffi.notes_core.NewNote
import uniffi.notes_core.Scope

/**
 * The debug scenarios, a step at a time: `adb shell am broadcast -n org.evsyukov.notes/.DebugReceiver --es step <step>`.
 * A step changes the data or the state of the running app, so that a picture of the screen can be taken.
 * Debug builds only: a release has no such receiver.
 */
class DebugReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val step = intent.getStringExtra("step").orEmpty()
        val done = goAsync()
        Repo.scope.launch(Repo.core) {
            run(step)
            done.finish()
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
            "justify" -> {
                Repo.store.setSettings(Repo.store.settings().copy(align = uniffi.notes_core.TextAlign.JUSTIFY, fontBody = "Literata"))
                Repo.changed()
            }
            "left" -> {
                Repo.store.setSettings(Repo.store.settings().copy(align = uniffi.notes_core.TextAlign.LEFT, fontBody = "system"))
                Repo.changed()
            }
            "long" -> {
                val lines = value.toInt()
                val text = buildString {
                    append("MEASURE\n\n")
                    repeat(lines) { append("Строка номер $it с **жирным** словом и `кодом` внутри абзаца.\n\n") }
                }
                val note = Repo.store.createNote(NewNote("Длинная $lines", null, emptyList(), text))
                Repo.changed()
                onMain { AppModel.open(note.id) }
            }
            // S40: an edit of the open note that comes from elsewhere, at its start.
            "remote" -> {
                val id = withContext(Dispatchers.Main) { AppModel.openNote } ?: return
                Repo.store.spliceUtf16(id, 0u, 0u, value)
                Repo.changed()
            }
        }
    }

    private companion object {
        val PROSE = "# Заголовок\n\nВозможности человеческого воображения и взаимодействие представителей разнообразных направлений " +
            "предоставляют возможность рассматривать в лес и в поле идти — дальше. Internationalization and characteristically " +
            "extraordinary considerations demonstrate hyphenation in English paragraphs of a narrow column.\n"

        /** Every construct of the dialect (E1–E16). */
        val DIALECT = """
            # Заголовок первого уровня

            Абзац с **жирным**, _курсивом_, ~~зачёркнутым~~, ==выделенным== и `кодом`, и [ссылка](https://example.com/page) в конце. Длинная строка, чтобы увидеть перенос и в лес идти — дальше.

            ## Второй уровень

            - первый пункт длинный настолько, чтобы перенестись на вторую строку экрана телефона
            - второй пункт
              - вложенный

            1. раз
            2. два

            - [ ] задача
            - [x] сделанная задача

            > Цитата в две
            > строки

            ---

            ```kotlin
            fun main() {
                println("привет") // комментарий
            }
            ```

            | Имя | Число |
            |-----|------:|
            | а   | 1     |
            | бб  | 22    |

            Последний абзац.
        """.trimIndent()
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
        store.createNote(NewNote("Разметка", null, emptyList(), DIALECT))
        store.createNote(NewNote("Перенос", null, emptyList(), PROSE))
        val gone = store.createNote(NewNote("Удалённая заметка", null, emptyList(), "текст"))
        store.deleteNote(gone.id)
        Repo.changed()
    }
}
