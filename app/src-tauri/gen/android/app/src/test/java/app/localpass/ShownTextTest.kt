package app.localpass

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** What a website chose to call itself or its account is shortened and cleaned before it is shown. */
class ShownTextTest {
    @Test
    fun shortNamesAreKept() {
        assertEquals("Alice Smith", ShownText.clip("Alice Smith"))
    }

    @Test
    fun longNamesAreCutWithAnEllipsis() {
        val shown = ShownText.clip("x".repeat(500))
        assertEquals(ShownText.NAME_MAX, shown.length)
        assertTrue(shown.endsWith("…"))
    }

    @Test
    fun lineBreaksAndControlCharactersCannotAddLines() {
        assertEquals("Example Bank Your account is locked", ShownText.clip("Example Bank\n\nYour account\u0000 is   locked"))
    }

    @Test
    fun lookAlikeDirectionTricksAreRemoved() {
        val shown = ShownText.clip("alice‮gnp.exe​⁦x")
        assertFalse(shown.any { it in '‪'..'‮' || it == '​' || it in '⁦'..'⁩' })
    }

    @Test
    fun aCutNeverSplitsASurrogatePair() {
        val shown = ShownText.clip("a".repeat(ShownText.NAME_MAX - 2) + "😀😀")
        assertFalse(shown.dropLast(1).lastOrNull()?.isHighSurrogate() == true)
    }

    @Test
    fun aLongSiteKeepsItsEndWhereTheRealDomainIs() {
        val host = "example.com." + "a".repeat(80) + ".evil.test"
        val shown = ShownText.site(host)
        assertEquals(ShownText.SITE_MAX, shown.length)
        assertTrue(shown.startsWith("…") && shown.endsWith(".evil.test"))
    }

    @Test
    fun anOrdinarySiteIsShownWhole() {
        assertEquals("login.example.com", ShownText.site("login.example.com"))
    }
}
