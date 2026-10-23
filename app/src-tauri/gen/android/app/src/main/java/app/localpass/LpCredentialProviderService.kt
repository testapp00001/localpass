package app.localpass

import android.content.Intent
import android.net.Uri
import android.os.CancellationSignal
import android.os.OutcomeReceiver
import androidx.annotation.RequiresApi
import androidx.credentials.exceptions.ClearCredentialException
import androidx.credentials.exceptions.CreateCredentialException
import androidx.credentials.exceptions.CreateCredentialUnknownException
import androidx.credentials.exceptions.GetCredentialException
import androidx.credentials.exceptions.GetCredentialUnknownException
import androidx.credentials.provider.BeginCreateCredentialRequest
import androidx.credentials.provider.BeginCreateCredentialResponse
import androidx.credentials.provider.BeginCreatePublicKeyCredentialRequest
import androidx.credentials.provider.BeginGetCredentialRequest
import androidx.credentials.provider.BeginGetCredentialResponse
import androidx.credentials.provider.CreateEntry
import androidx.credentials.provider.CredentialProviderService
import androidx.credentials.provider.ProviderClearCredentialStateRequest
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors
import org.json.JSONObject

/**
 * LocalPass as a passkey provider for Android's Credential Manager (Android 14+). Browsers that
 * Google lists as trusted report the website a request is for; for those, LocalPass offers its
 * passkeys and can create new ones. The service only builds the entries the system shows: the
 * private keys never leave the engine and signing happens in [CredentialActivity].
 */
@RequiresApi(34)
class LpCredentialProviderService : CredentialProviderService() {
    private val io: ExecutorService = Executors.newSingleThreadExecutor()

    override fun onDestroy() {
        io.shutdown()
        super.onDestroy()
    }

    override fun onBeginCreateCredentialRequest(
        request: BeginCreateCredentialRequest,
        cancellationSignal: CancellationSignal,
        callback: OutcomeReceiver<BeginCreateCredentialResponse, CreateCredentialException>,
    ) {
        try {
            val builder = BeginCreateCredentialResponse.Builder()
            // Passwords go through autofill; only passkeys are created here, and only for a site a
            // trusted browser vouches for.
            val origin = if (request is BeginCreatePublicKeyCredentialRequest) LpProvider.origin(this, request.callingAppInfo) else null
            if (request is BeginCreatePublicKeyCredentialRequest && origin != null) {
                val json = JSONObject(request.requestJson)
                val user = ShownText.clip(json.optJSONObject("user")?.optString("name").orEmpty())
                // The website the browser reported, not the name the page gave itself.
                val site = ShownText.site(Uri.parse(origin).host.orEmpty())
                val intent = Intent(this, CredentialActivity::class.java).setAction(CredentialActivity.ACTION_CREATE)
                val entry = CreateEntry.Builder("LocalPass", PasskeyEntries.pending(this, 2, intent))
                    .setDescription(listOf("Save a passkey", site.takeIf { it.isNotBlank() }?.let { "for $it" }, user.takeIf { it.isNotBlank() }?.let { "($it)" }).filterNotNull().joinToString(" "))
                    .build()
                builder.addCreateEntry(entry)
            }
            callback.onResult(builder.build())
        } catch (e: Exception) {
            callback.onError(CreateCredentialUnknownException(e.message))
        }
    }

    override fun onBeginGetCredentialRequest(
        request: BeginGetCredentialRequest,
        cancellationSignal: CancellationSignal,
        callback: OutcomeReceiver<BeginGetCredentialResponse, GetCredentialException>,
    ) {
        // Listing passkeys calls into the engine; keep it off the main thread.
        io.execute {
            try {
                callback.onResult(PasskeyEntries.response(this, request))
            } catch (e: Exception) {
                callback.onError(GetCredentialUnknownException(e.message))
            }
        }
    }

    override fun onClearCredentialStateRequest(
        request: ProviderClearCredentialStateRequest,
        cancellationSignal: CancellationSignal,
        callback: OutcomeReceiver<Void?, ClearCredentialException>,
    ) {
        // LocalPass keeps no per-app sign-in state to clear.
        callback.onResult(null)
    }
}
