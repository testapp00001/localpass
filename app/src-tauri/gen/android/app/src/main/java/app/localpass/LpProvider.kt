package app.localpass

import android.content.Context
import androidx.credentials.provider.CallingAppInfo
import org.json.JSONArray
import org.json.JSONObject

/**
 * What the credential services (Credential Manager provider, autofill) share: calls into the Rust
 * engine, and working out which website a request is for.
 */
internal object LpProvider {
    /** An answer from the engine: a value, or an error code with a message. */
    class Reply(private val json: JSONObject) {
        val ok: Boolean get() = !json.has("error")
        val code: String get() = json.optJSONObject("error")?.optString("code") ?: ""
        val message: String get() = json.optJSONObject("error")?.optString("message") ?: ""
        fun obj(): JSONObject = json.optJSONObject("ok") ?: JSONObject()
        fun array(): JSONArray = json.optJSONArray("ok") ?: JSONArray()
    }

    /** Blocking (the vault may run Argon2): call from a background thread. */
    fun call(context: Context, op: String, vararg fields: Pair<String, Any?>): Reply {
        val request = JSONObject().put("op", op)
        for ((key, value) in fields) request.put(key, value)
        val secret = try {
            DeviceSecretStore.get(context)
        } catch (e: DeviceSecretStore.Unavailable) {
            // Android's key store is not answering; the vault is untouched and the request can be repeated.
            return Reply(JSONObject().put("error", JSONObject().put("code", "keyStoreUnavailable").put("message", e.message)))
        }
        val reply = NativeBridge.credentialCall(context.filesDir.absolutePath, secret, request.toString())
        return Reply(JSONObject(reply))
    }

    @Volatile
    private var allowlist: String? = null

    /**
     * Google's list of browsers trusted to say which website a request comes from. It is bundled
     * with the app; nothing is downloaded.
     */
    fun privilegedAllowlist(context: Context): String =
        allowlist ?: context.assets.open("privileged_browsers.json").bufferedReader().use { it.readText() }
            .also { allowlist = it }

    /**
     * The website a request is for, but only when a trusted browser reports it. Other callers (a
     * native app) have no checked website, so they get nothing from the vault.
     */
    fun origin(context: Context, caller: CallingAppInfo?): String? {
        if (caller == null) return null
        return try {
            caller.getOrigin(privilegedAllowlist(context))?.takeIf { it.isNotBlank() }
        } catch (e: Exception) {
            null
        }
    }
}
