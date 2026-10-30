package org.evsyukov.notes.editor

import uniffi.notes_core.TextAlign

/** E44: the width of the column, a setting of the device. */
enum class ColumnWidth(val characters: Int?) {
    NARROW(60),
    MEDIUM(72),
    WIDE(90),
    FULL(null),
}

/** E40–E48: the settings of typography and what they come to. */
object Typography {
    /** E41: the size on this device, 12 to 28 points; nothing saved is the body size of the platform. */
    val sizes = 12..28

    fun size(saved: Int?, system: Int): Int = saved?.coerceIn(sizes) ?: system

    /** E40: the fonts on offer, as the settings name them. */
    val textFonts = listOf("system", "Literata", "Inter", "iA Writer Quattro")
    val codeFonts = listOf("system", "JetBrains Mono")

    /** E42, E43: the steps of the sliders. */
    val lineHeights = (12..20).map { it / 10.0 }
    val paragraphSpacings = (0..6).map { it * 0.25 }

    /** E45: justified text turns hyphenation on; with left alignment it is a switch of its own. */
    fun hyphenates(align: TextAlign, hyphenate: Boolean): Boolean = align == TextAlign.JUSTIFY || hyphenate

    /** E46: the language of a paragraph: Russian for Cyrillic text, English for Latin. */
    fun language(paragraph: CharSequence): String {
        val cyrillic = paragraph.count { it in 'Ѐ'..'ӿ' }
        val latin = paragraph.count { it in 'a'..'z' || it in 'A'..'Z' }
        return if (cyrillic > latin) "ru" else "en"
    }

    /**
     * E48: whether a line may break before the character at [index] of [text]: a one-letter Russian
     * word stays with the word after it, and an em dash with the word before it.
     */
    fun mayBreak(text: CharSequence, index: Int): Boolean {
        if (index <= 0 || index >= text.length) return true
        if (text[index] == '—') return false
        // What stands before the break, without the spaces: one letter after a space or the line start?
        var end = index
        while (end > 0 && text[end - 1].isBreakingOrNot()) end--
        if (end <= 0 || end >= index) return true
        if (text[end - 1] !in "авикосуяАВИКОСУЯ") return true
        return end >= 2 && !(text[end - 2].isBreakingOrNot() || text[end - 2] == '\n')
    }

    private fun Char.isBreakingOrNot() = this == ' ' || this == ' '
}
