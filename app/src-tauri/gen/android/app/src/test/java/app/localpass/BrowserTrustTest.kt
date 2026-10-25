package app.localpass

import java.io.File
import java.security.MessageDigest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The trusted-browser decision for autofill (no Android needed: it is a pure function of the list). */
class BrowserTrustTest {
    private val chromeRelease = "F0:FD:6C:5B:41:0F:25:CB:25:C3:B5:33:46:C8:97:2F:AE:30:F8:EE:74:11:DF:91:04:80:AD:6B:2D:60:DB:83"
    private val chromeReleaseHex = BrowserTrust.normalize(chromeRelease)
    private val stranger = "00".repeat(32)

    /** The list that ships in the app, so a broken or emptied asset fails here. */
    private val shipped by lazy {
        BrowserTrust.parse(File("src/main/assets/privileged_browsers.json").readText())
    }

    /**
     * The shipped list is Google's, fetched from https://www.gstatic.com/gpm-passkeys-privileged-apps/apps.json
     * on 2026-10-23 (byte for byte the same file then). It decides which browsers may tell LocalPass what
     * website a page belongs to, so changing it is a decision, not an accident: fetch it again, read the diff
     * (packages and certificates added or dropped), then update this digest and the date above. Carriage
     * returns are ignored so a checkout with Windows line endings gives the same digest.
     */
    @Test
    fun theShippedListIsThePinnedCopyOfGooglesList() {
        val bytes = File("src/main/assets/privileged_browsers.json").readBytes()
            .filter { it != '\r'.code.toByte() }.toByteArray()
        val digest = MessageDigest.getInstance("SHA-256").digest(bytes).joinToString("") { "%02x".format(it) }
        assertEquals("0891ee2507894999ead3eb4f42e17936a2e8ed8f8fd95592535c9811ef319481", digest)
    }

    @Test
    fun theShippedListHasChromeUnderItsPackageAndCertificate() {
        assertTrue(shipped.size > 5)
        assertTrue(chromeReleaseHex in shipped.getValue("com.android.chrome"))
    }

    @Test
    fun aListedBrowserWithItsListedCertificateIsTrusted() {
        assertTrue(BrowserTrust.isTrusted(shipped, "com.android.chrome", listOf(chromeReleaseHex)))
    }

    @Test
    fun aFingerprintIsTheSameWithColonsOrWithout() {
        assertTrue(BrowserTrust.isTrusted(shipped, "com.android.chrome", listOf(chromeRelease)))
        assertTrue(BrowserTrust.isTrusted(shipped, "com.android.chrome", listOf(chromeRelease.lowercase())))
    }

    @Test
    fun anyOneOfSeveralSigningCertificatesIsEnough() {
        assertTrue(BrowserTrust.isTrusted(shipped, "com.android.chrome", listOf(stranger, chromeReleaseHex)))
    }

    @Test
    fun anAppThatIsNotOnTheListIsRefused() {
        assertFalse(BrowserTrust.isTrusted(shipped, "com.example.notabrowser", listOf(chromeReleaseHex)))
        assertFalse(BrowserTrust.isTrusted(shipped, "app.localpass", listOf(chromeReleaseHex)))
    }

    @Test
    fun aListedPackageNameWithAnotherCertificateIsRefused() {
        assertFalse(BrowserTrust.isTrusted(shipped, "com.android.chrome", listOf(stranger)))
    }

    @Test
    fun aListedCertificateOnAnotherPackageIsRefused() {
        // Chrome's certificate does not make just any package a browser, nor another listed browser Chrome.
        assertFalse(BrowserTrust.isTrusted(shipped, "com.android.chrome.evil", listOf(chromeReleaseHex)))
        assertFalse(BrowserTrust.isTrusted(shipped, "com.chrome.beta", listOf(chromeReleaseHex)))
    }

    @Test
    fun noPackageOrNoCertificateIsRefused() {
        assertFalse(BrowserTrust.isTrusted(shipped, null, listOf(chromeReleaseHex)))
        assertFalse(BrowserTrust.isTrusted(shipped, "", listOf(chromeReleaseHex)))
        assertFalse(BrowserTrust.isTrusted(shipped, "com.android.chrome", emptyList()))
    }

    @Test
    fun aListThatCannotBeReadTrustsNothing() {
        for (broken in listOf("", "not json", "{}", """{"apps": 3}""", """{"apps": [null, 5, {}]}""")) {
            val allowed = BrowserTrust.parse(broken)
            assertTrue("parsed $broken", allowed.isEmpty())
            assertFalse(BrowserTrust.isTrusted(allowed, "com.android.chrome", listOf(chromeReleaseHex)))
        }
    }

    @Test
    fun onlyAndroidEntriesWithAFullFingerprintCount() {
        val json = """{"apps": [
            {"type": "web", "info": {"package_name": "com.web.thing", "signatures": [{"cert_fingerprint_sha256": "$chromeRelease"}]}},
            {"type": "android", "info": {"package_name": "com.short.print", "signatures": [{"cert_fingerprint_sha256": "AB:CD"}]}},
            {"type": "android", "info": {"package_name": "com.good.browser", "signatures": [{"cert_fingerprint_sha256": "$chromeRelease"}]}}
        ]}"""
        assertEquals(setOf("com.good.browser"), BrowserTrust.parse(json).keys)
    }

    @Test
    fun developerBuildKeysCountOnlyOnADeveloperBuildOfAndroid() {
        val testKey = "C8:A2:E9:BC:CF:59:7C:2F:B6:DC:66:E3:77:10:DB:9B:83:FE:62:6A:E0:7A:36:3C:9D:B3:F7:E4:19:2A:B8:00"
        val json = """{"apps": [
            {"type": "android", "info": {"package_name": "com.some.browser", "signatures": [
                {"build": "release", "cert_fingerprint_sha256": "$chromeRelease"},
                {"build": "userdebug", "cert_fingerprint_sha256": "$testKey"}]}}
        ]}"""
        val phone = BrowserTrust.parse(json)
        assertTrue(BrowserTrust.isTrusted(phone, "com.some.browser", listOf(chromeReleaseHex)))
        assertFalse("a userdebug key must not make an app a browser", BrowserTrust.isTrusted(phone, "com.some.browser", listOf(testKey)))
        val developerPhone = BrowserTrust.parse(json, allowUserdebug = true)
        assertTrue(BrowserTrust.isTrusted(developerPhone, "com.some.browser", listOf(testKey)))
    }

    @Test
    fun noUserdebugKeyOfTheShippedListIsTrustedOnAnOrdinaryPhone() {
        val root = org.json.JSONObject(File("src/main/assets/privileged_browsers.json").readText())
        var seen = 0
        for (i in 0 until root.getJSONArray("apps").length()) {
            val info = root.getJSONArray("apps").getJSONObject(i).optJSONObject("info") ?: continue
            val signatures = info.optJSONArray("signatures") ?: continue
            for (j in 0 until signatures.length()) {
                val signature = signatures.getJSONObject(j)
                if (signature.optString("build") != "userdebug") continue
                seen++
                val name = info.getString("package_name")
                val fingerprint = signature.getString("cert_fingerprint_sha256")
                val releaseKeys = (0 until signatures.length()).map { signatures.getJSONObject(it) }
                    .filter { it.optString("build") != "userdebug" }
                    .map { BrowserTrust.normalize(it.getString("cert_fingerprint_sha256")) }
                // A key listed both ways would be fine; one listed only as userdebug must be refused.
                if (BrowserTrust.normalize(fingerprint) !in releaseKeys) {
                    assertFalse("$name userdebug key", BrowserTrust.isTrusted(shipped, name, listOf(fingerprint)))
                }
            }
        }
        assertTrue("the shipped list has userdebug entries, so this test means something", seen > 0)
    }

    @Test
    fun theDigestOfACertificateIsLowerCaseHex() {
        assertEquals("e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855", BrowserTrust.sha256(ByteArray(0)))
    }
}
