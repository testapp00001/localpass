package app.localpass

import android.app.assist.AssistStructure
import android.content.Context
import android.content.pm.PackageManager
import android.os.Build
import android.util.Log
import java.security.MessageDigest
import org.json.JSONObject

/**
 * Which apps may tell LocalPass the website a login form belongs to. Android's autofill reports a
 * `webDomain` for any view that says so, and any app can put a WebView on screen that claims to be
 * any site, so the claim is only believed when the app in front is a browser on Google's list of
 * trusted browsers (`assets/privileged_browsers.json`, the same list the passkey path uses): its
 * package name must be listed and the certificate it is signed with must be one of the listed
 * fingerprints. Any other app gets no logins and no save offer.
 *
 * What this does not do: the browser still decides what it reports. A trusted browser vouches for
 * the page's domain; LocalPass cannot check that independently.
 */
internal object BrowserTrust {
    private const val TAG = "LpBrowserTrust"

    /** "F0:FD:6C..." and "f0fd6c..." are the same fingerprint. */
    fun normalize(fingerprint: String): String = fingerprint.filter { it.isLetterOrDigit() }.lowercase()

    fun hex(bytes: ByteArray): String = bytes.joinToString("") { "%02x".format(it) }

    fun sha256(certificate: ByteArray): String = hex(MessageDigest.getInstance("SHA-256").digest(certificate))

    /**
     * Reads Google's list format: `{"apps": [{"type": "android", "info": {"package_name": ...,
     * "signatures": [{"cert_fingerprint_sha256": ..., "build": "release"}]}}]}`. Anything else in it
     * is skipped, and a file that cannot be read is an empty list, which trusts nothing.
     *
     * Signatures marked `"build": "userdebug"` are the keys that browsers are signed with on
     * developer builds of Android, and some of them are public test keys. Like the Credential Manager
     * library, this counts them only on a phone that is itself a userdebug build (`allowUserdebug`);
     * on any other phone, an app signed with one of them is not a browser (phase 5.3, RD-1).
     */
    fun parse(json: String, allowUserdebug: Boolean = false): Map<String, Set<String>> {
        val allowed = mutableMapOf<String, MutableSet<String>>()
        try {
            val apps = JSONObject(json).optJSONArray("apps") ?: return emptyMap()
            for (i in 0 until apps.length()) {
                val app = apps.optJSONObject(i) ?: continue
                if (app.optString("type") != "android") continue
                val info = app.optJSONObject("info") ?: continue
                val name = info.optString("package_name")
                val signatures = info.optJSONArray("signatures") ?: continue
                if (name.isBlank()) continue
                for (j in 0 until signatures.length()) {
                    val signature = signatures.optJSONObject(j)
                    if (!allowUserdebug && signature?.optString("build") == "userdebug") continue
                    val fingerprint = normalize(signature?.optString("cert_fingerprint_sha256").orEmpty())
                    if (fingerprint.length == 64) allowed.getOrPut(name) { mutableSetOf() }.add(fingerprint)
                }
            }
        } catch (e: Exception) {
            return emptyMap()
        }
        return allowed
    }

    /**
     * The decision, with no Android in it so it can be tested on its own: the package is listed
     * and at least one certificate it is signed with (as SHA-256 hex) is listed for that package.
     */
    fun isTrusted(allowed: Map<String, Set<String>>, packageName: String?, certificateSha256: Collection<String>): Boolean {
        if (packageName.isNullOrBlank()) return false
        val listed = allowed[packageName] ?: return false
        return certificateSha256.any { normalize(it) in listed }
    }

    @Volatile
    private var parsed: Map<String, Set<String>>? = null

    /**
     * The bundled list, read and parsed once: package name to the SHA-256 fingerprints (lower-case
     * hex, no colons) of the certificates it may be signed with.
     */
    private fun allowlist(context: Context): Map<String, Set<String>> =
        parsed ?: parse(LpProvider.privilegedAllowlist(context), allowUserdebug = Build.TYPE == "userdebug").also { parsed = it }

    /** The certificates the installed package is signed with right now (SHA-256 hex). */
    @Suppress("DEPRECATION")
    private fun certificates(context: Context, packageName: String): List<String> {
        val packages = context.packageManager
        val signers = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            packages.getPackageInfo(packageName, PackageManager.GET_SIGNING_CERTIFICATES).signingInfo?.apkContentsSigners
        } else {
            packages.getPackageInfo(packageName, PackageManager.GET_SIGNATURES).signatures
        }
        return signers.orEmpty().map { sha256(it.toByteArray()) }
    }

    /** Whether [packageName] is an installed browser on the list, signed as listed. Blocking-cheap; fails closed. */
    fun isTrustedPackage(context: Context, packageName: String?): Boolean {
        if (packageName.isNullOrBlank()) return false
        val trusted = try {
            isTrusted(allowlist(context), packageName, certificates(context, packageName))
        } catch (e: Exception) {
            // Not installed, not visible, or the list is unreadable: no trust.
            false
        }
        if (!trusted) Log.i(TAG, "not a trusted browser: $packageName")
        return trusted
    }

    /** The app whose screen [structure] describes: the activity the system took it from. */
    fun packageOf(structure: AssistStructure?): String? = structure?.activityComponent?.packageName
}
