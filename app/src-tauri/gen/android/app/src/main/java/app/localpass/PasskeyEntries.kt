package app.localpass

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.graphics.drawable.Icon
import androidx.annotation.RequiresApi
import androidx.credentials.provider.AuthenticationAction
import androidx.credentials.provider.BeginGetCredentialRequest
import androidx.credentials.provider.BeginGetCredentialResponse
import androidx.credentials.provider.BeginGetPublicKeyCredentialOption
import androidx.credentials.provider.CredentialEntry
import androidx.credentials.provider.PublicKeyCredentialEntry
import java.time.Instant

/** What the system's passkey picker shows for LocalPass, and what happens when one is chosen. */
@RequiresApi(34)
internal object PasskeyEntries {
    /** A PendingIntent the system can fill in with the request details when the user picks. */
    fun pending(context: Context, code: Int, intent: Intent): PendingIntent =
        PendingIntent.getActivity(context, code, intent, PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT)

    private fun activity(context: Context, action: String) =
        Intent(context, CredentialActivity::class.java).setAction(action)

    /**
     * The answer to a sign-in request. When the vault is locked it is a single "Unlock" action (the
     * passkeys can't be listed yet); otherwise it lists the passkeys for the site. [unlockToken] is
     * set only by the picker's own unlock step: it rides in the entries so that the one passkey the
     * user picks next needs no second check (see [UnlockCarry]).
     */
    fun response(context: Context, request: BeginGetCredentialRequest, unlockToken: String? = null): BeginGetCredentialResponse {
        val builder = BeginGetCredentialResponse.Builder()
        // Only a trusted browser says which site this is; without that there is nothing safe to offer.
        val origin = LpProvider.origin(context, request.callingAppInfo) ?: return builder.build()
        val options = request.beginGetCredentialOptions.filterIsInstance<BeginGetPublicKeyCredentialOption>()
        if (options.isEmpty()) return builder.build()

        val status = LpProvider.call(context, "status")
        if (!status.ok || status.obj().optString("status") == "noVault") return builder.build()
        if (status.obj().optString("status") != "unlocked") {
            val unlock = pending(context, 1, activity(context, CredentialActivity.ACTION_UNLOCK))
            return builder.setAuthenticationActions(listOf(AuthenticationAction("Unlock LocalPass", unlock))).build()
        }

        val entries = mutableListOf<CredentialEntry>()
        for (option in options) {
            val list = LpProvider.call(context, "passkeyList", "origin" to origin, "optionsJson" to option.requestJson)
            if (!list.ok) continue
            val candidates = list.array()
            for (i in 0 until candidates.length()) {
                val c = candidates.getJSONObject(i)
                val itemId = c.getString("itemId")
                val intent = activity(context, CredentialActivity.ACTION_GET).putExtra(CredentialActivity.EXTRA_ITEM_ID, itemId)
                if (unlockToken != null) intent.putExtra(CredentialActivity.EXTRA_UNLOCK_TOKEN, unlockToken)
                // The site is the relying party id the engine matched to the browser's origin; the names are the site's own words, shortened.
                val site = ShownText.site(c.optString("rpId"))
                val name = ShownText.clip(c.optString("userName").ifBlank { c.optString("userDisplayName") }).ifBlank { site }
                val entry = PublicKeyCredentialEntry.Builder(
                    context,
                    name,
                    pending(context, itemId.hashCode() and 0x7fffffff, intent),
                    option,
                )
                    .setDisplayName(site)
                    .setIcon(Icon.createWithResource(context, R.mipmap.ic_launcher))
                    .setLastUsedTime(Instant.ofEpochMilli(c.optLong("createdMs")))
                    .build()
                entries.add(entry)
            }
        }
        return builder.setCredentialEntries(entries).build()
    }
}
