package app.localpass

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

/**
 * The FileProvider is only used to hand the system camera one file path in the app's own picture
 * folder (the web view's file chooser). It must not make all of external storage or the whole cache
 * available to whoever gets a grant.
 */
class FileProviderPathsTest {
    private val xml = File("src/main/res/xml/file_paths.xml").readText()

    @Test
    fun allOfExternalStorageAndTheCacheAreNotShared() {
        assertFalse("external-path covers the whole card", Regex("<external-path\\b").containsMatchIn(xml))
        assertFalse("cache-path covers the whole cache", Regex("<cache-path\\b").containsMatchIn(xml))
        assertFalse("a whole root", Regex("path=\"\\.?/?\"").containsMatchIn(xml))
    }

    @Test
    fun onlyThePictureFolderOfTheAppIsShared() {
        val entries = Regex("<([a-z-]+)\\s[^>]*path=\"([^\"]*)\"").findAll(xml).map { it.groupValues[1] to it.groupValues[2] }.toList()
        assertEquals(listOf("external-files-path" to "Pictures/"), entries)
    }
}
