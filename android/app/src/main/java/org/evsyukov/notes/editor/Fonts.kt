package org.evsyukov.notes.editor

import android.content.res.AssetManager
import android.graphics.Typeface

/** The four faces of a font (E40). */
class FontSet(val regular: Typeface, val bold: Typeface, val italic: Typeface, val boldItalic: Typeface) {
    fun pick(bold: Boolean, italic: Boolean): Typeface = when {
        bold && italic -> boldItalic
        bold -> this.bold
        italic -> this.italic
        else -> regular
    }

    fun holds(face: Typeface?): Boolean = face === regular || face === bold || face === italic || face === boldItalic
}

/** E40: the fonts built in, from the same files as the macOS app. */
object Fonts {
    private val loaded = mutableMapOf<String, FontSet>()

    private fun variable(assets: AssetManager, file: String, weight: Int, italic: Boolean): Typeface =
        Typeface.Builder(assets, "fonts/$file.ttf")
            .setFontVariationSettings("'wght' $weight")
            .setWeight(weight)
            .setItalic(italic)
            .build()

    private fun files(name: String): String? = when (name) {
        "Literata" -> "Literata"
        "Inter" -> "Inter"
        "iA Writer Quattro" -> "iAWriterQuattro"
        "JetBrains Mono" -> "JetBrainsMono"
        else -> null
    }

    private fun system(base: Typeface) = FontSet(
        Typeface.create(base, Typeface.NORMAL),
        Typeface.create(base, Typeface.BOLD),
        Typeface.create(base, Typeface.ITALIC),
        Typeface.create(base, Typeface.BOLD_ITALIC),
    )

    fun text(assets: AssetManager, name: String): FontSet = set(assets, name, Typeface.DEFAULT)

    fun code(assets: AssetManager, name: String): FontSet = set(assets, name, Typeface.MONOSPACE)

    private fun set(assets: AssetManager, name: String, fallback: Typeface): FontSet = loaded.getOrPut("$name/${fallback.hashCode()}") {
        val file = files(name) ?: return@getOrPut system(fallback)
        runCatching {
            FontSet(
                variable(assets, file, 400, false),
                variable(assets, file, 700, false),
                variable(assets, "$file-Italic", 400, true),
                variable(assets, "$file-Italic", 700, true),
            )
        }.getOrElse { system(fallback) }
    }
}
