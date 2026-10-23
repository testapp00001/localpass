package app.localpass

/**
 * Text that a website chose (its name, the account name) and that LocalPass puts on a system sheet
 * or a prompt. It is shortened and stripped of line breaks and look-alike tricks so it cannot
 * push the real site name off the screen or change the order of what is read. The relying party id
 * is what the passkey is tied to, so it is the one shown prominently; names are secondary.
 */
internal object ShownText {
    const val NAME_MAX = 40
    const val SITE_MAX = 48

    /** Control characters, zero-width characters and the bidirectional overrides and isolates. */
    private fun invisible(ch: Char): Boolean =
        ch.isISOControl() || ch in '​'..'‏' || ch in '‪'..'‮' || ch in '⁠'..'⁤' ||
            ch in '⁦'..'⁩' || ch == '﻿'

    private fun clean(text: String): String {
        val kept = StringBuilder(text.length)
        for (ch in text) {
            when {
                ch == '\n' || ch == '\r' || ch == '\t' -> kept.append(' ')
                invisible(ch) -> Unit
                else -> kept.append(ch)
            }
        }
        return kept.toString().trim().replace(Regex(" {2,}"), " ")
    }

    /** A name or account the page supplied: at most [max] characters, with "…" when cut. */
    fun clip(text: String, max: Int = NAME_MAX): String {
        val cleaned = clean(text)
        if (cleaned.length <= max) return cleaned
        var cut = cleaned.substring(0, max - 1).trimEnd()
        if (cut.isNotEmpty() && cut.last().isHighSurrogate()) cut = cut.dropLast(1)
        return "$cut…"
    }

    /**
     * A website name. A long one loses its beginning, not its end: the registrable domain is at
     * the right, and `example.com.aaaa…evil.test` must not show as `example.com.aaaa…`.
     */
    fun site(host: String, max: Int = SITE_MAX): String {
        val cleaned = clean(host)
        if (cleaned.length <= max) return cleaned
        var cut = cleaned.substring(cleaned.length - (max - 1))
        if (cut.isNotEmpty() && cut.first().isLowSurrogate()) cut = cut.drop(1)
        return "…$cut"
    }
}
