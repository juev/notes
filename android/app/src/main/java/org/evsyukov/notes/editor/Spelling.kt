package org.evsyukov.notes.editor

import android.content.Context
import android.text.Editable
import android.text.SpanWatcher
import android.text.Spannable
import android.text.Spanned
import android.text.style.SuggestionSpan
import android.view.textservice.SentenceSuggestionsInfo
import android.view.textservice.SpellCheckerSession
import android.view.textservice.SuggestionsInfo
import android.view.textservice.TextInfo
import android.view.textservice.TextServicesManager
import java.util.Locale

/**
 * E50, E51: the words the spell checker of the system marks, only in text. The view checks by itself only
 * the paragraphs edited since it opened; this asks the service for the rest, a session for each language,
 * and marks the words the way the view does, so that its menu of corrections works on them.
 */
class Spelling(private val context: Context) {
    private val manager = context.getSystemService(TextServicesManager::class.java)
    private val sessions = HashMap<String, SpellCheckerSession?>()
    private var excluded: List<IntRange> = emptyList()
    private val known = HashSet<String>()
    private var text: Editable? = null

    /** What the core keeps out of checking (E51). */
    private val filter = object : SpanWatcher {
        override fun onSpanAdded(spannable: Spannable, what: Any, start: Int, end: Int) {
            if (what is SuggestionSpan && isExcluded(start, end)) spannable.removeSpan(what)
        }

        override fun onSpanRemoved(spannable: Spannable, what: Any, start: Int, end: Int) = Unit

        override fun onSpanChanged(spannable: Spannable, what: Any, ostart: Int, oend: Int, nstart: Int, nend: Int) = Unit
    }

    private fun isExcluded(start: Int, end: Int) = excluded.any { start <= it.last && end > it.first }

    /** The text of a note that was just opened. */
    fun attach(value: Editable) {
        text = value
        known.clear()
        value.setSpan(filter, 0, value.length, Spanned.SPAN_INCLUSIVE_INCLUSIVE)
    }

    /** The ranges of the core for the text as it is now; a word marked before it became code loses its mark. */
    fun exclude(ranges: List<IntRange>) {
        excluded = ranges
        val value = text ?: return
        value.getSpans(0, value.length, SuggestionSpan::class.java).forEach {
            if (isExcluded(value.getSpanStart(it), value.getSpanEnd(it))) value.removeSpan(it)
        }
    }

    fun clear() {
        val value = text ?: return
        value.getSpans(0, value.length, SuggestionSpan::class.java).forEach(value::removeSpan)
        known.clear()
    }

    /** Asks about the paragraphs between two offsets that were not asked about in this form. [language] is `auto` or a language of the settings. */
    fun check(from: Int, to: Int, language: String) {
        val value = text ?: return
        val whole = value.toString()
        var start = whole.lastIndexOf('\n', (from - 1).coerceAtLeast(0)) + 1
        while (start < to.coerceAtMost(whole.length)) {
            val end = whole.indexOf('\n', start).let { if (it < 0) whole.length else it }
            val paragraph = whole.substring(start, end)
            if (paragraph.isNotBlank() && known.add(paragraph)) {
                val tag = if (language == "auto") Typography.language(paragraph) else language
                session(tag)?.let { ask(it, paragraph, start, tag) }
            }
            start = end + 1
        }
    }

    private class Asked(val paragraph: String, val start: Int)

    private val waiting = HashMap<Int, Asked>()
    private var nextCookie = 1

    private fun ask(session: SpellCheckerSession, paragraph: String, start: Int, tag: String) {
        val cookie = nextCookie++
        waiting[cookie] = Asked(paragraph, start)
        session.getSentenceSuggestions(arrayOf(TextInfo(paragraph, cookie, tag.hashCode())), SUGGESTIONS)
    }

    private fun session(tag: String): SpellCheckerSession? = sessions.getOrPut(tag) {
        if (manager == null || !manager.isSpellCheckerEnabled) return@getOrPut null
        manager.newSpellCheckerSession(null, Locale.forLanguageTag(tag), Answers(), false)
    }

    private inner class Answers : SpellCheckerSession.SpellCheckerSessionListener {
        override fun onGetSuggestions(results: Array<out SuggestionsInfo>?) = Unit

        override fun onGetSentenceSuggestions(results: Array<out SentenceSuggestionsInfo>?) {
            results?.filterNotNull()?.forEach(::mark)
        }
    }

    private fun mark(sentence: SentenceSuggestionsInfo) {
        val value = text ?: return
        for (index in 0 until sentence.suggestionsCount) {
            val info = sentence.getSuggestionsInfoAt(index) ?: continue
            val asked = waiting[info.cookie] ?: continue
            val wrong = info.suggestionsAttributes and (SuggestionsInfo.RESULT_ATTR_LOOKS_LIKE_TYPO or SuggestionsInfo.RESULT_ATTR_LOOKS_LIKE_GRAMMAR_ERROR) != 0
            if (!wrong) continue
            val offset = sentence.getOffsetAt(index)
            val length = sentence.getLengthAt(index)
            if (offset < 0 || offset + length > asked.paragraph.length) continue
            val start = asked.start + offset
            val end = start + length
            // The text may have moved on since the question; a word that is not there any more is left alone.
            if (end > value.length || !value.regionMatches(start, asked.paragraph, offset, length)) continue
            if (isExcluded(start, end) || value.getSpans(start, end, SuggestionSpan::class.java).isNotEmpty()) continue
            val words = Array(maxOf(0, info.suggestionsCount)) { info.getSuggestionAt(it) }
            value.setSpan(SuggestionSpan(context, words, SuggestionSpan.FLAG_MISSPELLED), start, end, Spanned.SPAN_EXCLUSIVE_EXCLUSIVE)
        }
    }

    fun close() {
        sessions.values.forEach { it?.close() }
        sessions.clear()
        text = null
    }

    private companion object {
        const val SUGGESTIONS = 5
    }
}
