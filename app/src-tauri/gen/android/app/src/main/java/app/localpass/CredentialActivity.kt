package app.localpass

import android.content.Intent
import android.net.Uri
import android.os.Bundle
import android.provider.Settings
import android.util.Base64
import android.view.WindowManager
import android.widget.Toast
import androidx.annotation.RequiresApi
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.credentials.CreateCredentialResponse
import androidx.credentials.CreatePublicKeyCredentialRequest
import androidx.credentials.CreatePublicKeyCredentialResponse
import androidx.credentials.GetCredentialResponse
import androidx.credentials.GetPublicKeyCredentialOption
import androidx.credentials.PublicKeyCredential
import androidx.credentials.exceptions.CreateCredentialCancellationException
import androidx.credentials.exceptions.CreateCredentialException
import androidx.credentials.exceptions.CreateCredentialUnknownException
import androidx.credentials.exceptions.GetCredentialCancellationException
import androidx.credentials.exceptions.GetCredentialException
import androidx.credentials.exceptions.GetCredentialUnknownException
import androidx.credentials.exceptions.domerrors.InvalidStateError
import androidx.credentials.exceptions.domerrors.NotAllowedError
import androidx.credentials.exceptions.domerrors.NotSupportedError
import androidx.credentials.exceptions.domerrors.SecurityError
import androidx.credentials.exceptions.publickeycredential.CreatePublicKeyCredentialDomException
import androidx.credentials.exceptions.publickeycredential.GetPublicKeyCredentialDomException
import androidx.credentials.provider.PendingIntentHandler
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * Where the system's passkey picker sends the user after they choose LocalPass: to create a
 * passkey, to sign in with one, or to unlock the vault so its passkeys can be listed. Each ends
 * with a fingerprint or screen-lock check (or the master password, when it just unlocked the
 * vault) before the engine creates or signs anything. A phone without a screen lock is refused
 * up front ([PasskeyPolicy]), so a passkey never claims a user check that did not happen, and
 * creating a passkey over an existing one for the same account asks first.
 */
@RequiresApi(34)
class CredentialActivity : AppCompatActivity() {
    private val io: ExecutorService = Executors.newSingleThreadExecutor()
    private lateinit var unlocker: Unlocker
    private var finished = false

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Keep passkey details out of screenshots and the recent-apps preview.
        window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        unlocker = Unlocker(this, io)
        when (intent.action) {
            ACTION_CREATE -> create()
            ACTION_GET -> get()
            ACTION_UNLOCK -> unlockThenList()
            else -> {
                setResult(RESULT_CANCELED)
                finish()
            }
        }
    }

    override fun onDestroy() {
        io.shutdown()
        super.onDestroy()
    }

    private fun toast(message: String) = Toast.makeText(this, message, Toast.LENGTH_LONG).show()

    private fun finishWith(result: Intent) {
        if (finished) return
        finished = true
        setResult(RESULT_OK, result)
        finish()
    }

    // ---- creating a passkey ----

    private fun finishCreate(response: CreateCredentialResponse?, exception: CreateCredentialException? = null) {
        val result = Intent()
        if (response != null) {
            PendingIntentHandler.setCreateCredentialResponse(result, response)
        } else {
            PendingIntentHandler.setCreateCredentialException(result, exception ?: CreateCredentialUnknownException())
        }
        finishWith(result)
    }

    private fun createError(code: String, message: String): CreateCredentialException = when (code) {
        "passkeyExists" -> CreatePublicKeyCredentialDomException(InvalidStateError(), message)
        "unsupported" -> CreatePublicKeyCredentialDomException(NotSupportedError(), message)
        "invalid" -> CreatePublicKeyCredentialDomException(SecurityError(), message)
        else -> CreateCredentialUnknownException(message)
    }

    private fun create() {
        val request = PendingIntentHandler.retrieveProviderCreateCredentialRequest(intent)
        val call = request?.callingRequest as? CreatePublicKeyCredentialRequest
        val origin = request?.let { LpProvider.origin(this, it.callingAppInfo) }
        if (call == null || origin == null) {
            toast("LocalPass can only save passkeys for websites opened in a supported browser.")
            return finishCreate(null, CreateCredentialUnknownException("unsupported caller"))
        }
        if (refuseWithoutScreenLock { finishCreate(null, CreateCredentialCancellationException()) }) return
        unlocker.unlock { outcome ->
            when (outcome) {
                Unlocker.Outcome.Cancelled -> finishCreate(null, CreateCredentialCancellationException())
                is Unlocker.Outcome.Failed -> {
                    toast(outcome.message)
                    finishCreate(null, CreateCredentialUnknownException(outcome.message))
                }
                // Only an unlock done here counts as the user check; an earlier unlock elsewhere does not.
                else -> checkThenCreate(origin, call.requestJson, outcome == Unlocker.Outcome.Unlocked)
            }
        }
    }

    private fun checkThenCreate(origin: String, requestJson: String, justVerified: Boolean) {
        io.execute {
            val check = LpProvider.call(this, "passkeyCheckCreate", "origin" to origin, "optionsJson" to requestJson)
            if (!check.ok) {
                runOnUiThread {
                    toast(check.message)
                    finishCreate(null, createError(check.code, check.message))
                }
                return@execute
            }
            // The website the engine checked against the browser's origin; the account is only what the page called it.
            val site = ShownText.site(check.obj().optString("rpId"))
            val account = ShownText.clip(check.obj().let { it.optString("userName").ifBlank { it.optString("userDisplayName") } })
            val replace = LpProvider.call(this, "passkeyWouldReplace", "origin" to origin, "optionsJson" to requestJson)
            if (!replace.ok) {
                runOnUiThread {
                    toast(replace.message)
                    finishCreate(null, createError(replace.code, replace.message))
                }
                return@execute
            }
            val replacing = !replace.obj().isNull("replaces")
            runOnUiThread {
                val verifyThenCreate = {
                    if (justVerified) {
                        createNow(origin, requestJson)
                    } else {
                        val title = if (replacing) "Replace your passkey for $site" else "Create a passkey for $site"
                        unlocker.verifyUser(title, account) { verified ->
                            when (verified) {
                                null -> finishCreate(null, CreateCredentialCancellationException())
                                false -> noScreenLockNow { finishCreate(null, CreateCredentialCancellationException()) }
                                true -> createNow(origin, requestJson)
                            }
                        }
                    }
                }
                if (replacing) {
                    confirmReplace(site, account) { yes -> if (yes) verifyThenCreate() else finishCreate(null, CreateCredentialCancellationException()) }
                } else {
                    verifyThenCreate()
                }
            }
        }
    }

    /** Asks before a new passkey takes the place of one the user already has for this account. */
    private fun confirmReplace(site: String, account: String, onAnswer: (Boolean) -> Unit) {
        val which = if (account.isBlank()) site else "$site / $account"
        val dialog = AlertDialog.Builder(this)
            .setTitle("Replace your passkey?")
            .setMessage("This replaces your existing passkey for $which. The old passkey will stop working.")
            .setPositiveButton("Replace") { _, _ -> onAnswer(true) }
            .setNegativeButton("Cancel") { _, _ -> onAnswer(false) }
            .setOnCancelListener { onAnswer(false) }
            .create()
        dialog.window?.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        dialog.show()
    }

    /** Only reached after the user was checked (fingerprint, screen lock or master password), so the passkey says so. */
    private fun createNow(origin: String, requestJson: String) {
        io.execute {
            val made = LpProvider.call(this, "passkeyCreate", "origin" to origin, "optionsJson" to requestJson, "userVerified" to true)
            runOnUiThread {
                if (made.ok) {
                    // The new passkey should reach the other devices soon.
                    SyncWorker.scheduleSoon(applicationContext)
                    finishCreate(CreatePublicKeyCredentialResponse(made.obj().getString("responseJson")))
                } else {
                    toast(made.message)
                    finishCreate(null, createError(made.code, made.message))
                }
            }
        }
    }

    // ---- signing in ----

    private fun finishGet(response: GetCredentialResponse?, exception: GetCredentialException? = null) {
        val result = Intent()
        if (response != null) {
            PendingIntentHandler.setGetCredentialResponse(result, response)
        } else {
            PendingIntentHandler.setGetCredentialException(result, exception ?: GetCredentialUnknownException())
        }
        finishWith(result)
    }

    private fun getError(code: String, message: String): GetCredentialException = when (code) {
        "invalid" -> GetPublicKeyCredentialDomException(SecurityError(), message)
        "notFound" -> GetPublicKeyCredentialDomException(NotAllowedError(), message)
        else -> GetCredentialUnknownException(message)
    }

    private fun get() {
        val request = PendingIntentHandler.retrieveProviderGetCredentialRequest(intent)
        val option = request?.credentialOptions?.filterIsInstance<GetPublicKeyCredentialOption>()?.firstOrNull()
        val origin = request?.let { LpProvider.origin(this, it.callingAppInfo) }
        val hash = option?.clientDataHash
        val itemId = intent.getStringExtra(EXTRA_ITEM_ID)
        if (option == null || origin == null || hash == null || itemId == null) {
            toast("LocalPass can only sign in to websites opened in a supported browser.")
            return finishGet(null, GetCredentialUnknownException("unsupported caller"))
        }
        val site = ShownText.site(Uri.parse(origin).host ?: origin)
        if (refuseWithoutScreenLock { finishGet(null, GetCredentialCancellationException()) }) return
        unlocker.unlock { outcome ->
            when (outcome) {
                Unlocker.Outcome.Cancelled -> finishGet(null, GetCredentialCancellationException())
                is Unlocker.Outcome.Failed -> {
                    toast(outcome.message)
                    finishGet(null, GetCredentialUnknownException(outcome.message))
                }
                Unlocker.Outcome.Unlocked -> signNow(origin, option.requestJson, itemId, hash)
                // The vault was already open. The user is checked once for this pick: by the unlock
                // step of this same picker (its token), otherwise here. An unlock anywhere else does not count.
                Unlocker.Outcome.AlreadyUnlocked ->
                    if (UnlockCarry.shared.redeem(intent.getStringExtra(EXTRA_UNLOCK_TOKEN), origin)) {
                        signNow(origin, option.requestJson, itemId, hash)
                    } else {
                        unlocker.verifyUser("Sign in to $site with a passkey", "") { verified ->
                            when (verified) {
                                null -> finishGet(null, GetCredentialCancellationException())
                                false -> noScreenLockNow { finishGet(null, GetCredentialCancellationException()) }
                                true -> signNow(origin, option.requestJson, itemId, hash)
                            }
                        }
                    }
            }
        }
    }

    /** Only reached after the user was checked, so the assertion says so. */
    private fun signNow(origin: String, requestJson: String, itemId: String, clientDataHash: ByteArray) {
        val hash = Base64.encodeToString(clientDataHash, Base64.URL_SAFE or Base64.NO_WRAP or Base64.NO_PADDING)
        io.execute {
            val signed = LpProvider.call(
                this, "passkeyGet",
                "origin" to origin, "optionsJson" to requestJson, "itemId" to itemId,
                "clientDataHash" to hash, "userVerified" to true,
            )
            runOnUiThread {
                if (signed.ok) {
                    finishGet(GetCredentialResponse(PublicKeyCredential(signed.obj().getString("responseJson"))))
                } else {
                    toast(signed.message)
                    finishGet(null, getError(signed.code, signed.message))
                }
            }
        }
    }

    // ---- unlocking so the passkeys can be listed ----

    private fun unlockThenList() {
        val begin = PendingIntentHandler.retrieveBeginGetCredentialRequest(intent)
        if (begin == null) {
            return finishGet(null, GetCredentialUnknownException("no request"))
        }
        if (refuseWithoutScreenLock { finishGet(null, GetCredentialCancellationException()) }) return
        unlocker.unlock { outcome ->
            when (outcome) {
                Unlocker.Outcome.Cancelled -> finishGet(null, GetCredentialCancellationException())
                is Unlocker.Outcome.Failed -> {
                    toast(outcome.message)
                    finishGet(null, GetCredentialUnknownException(outcome.message))
                }
                else -> io.execute {
                    // Only the unlock done here (not an already open vault) checked the user, and only
                    // for the passkey picked from this list: the token goes into these entries and nowhere else.
                    val origin = LpProvider.origin(this, begin.callingAppInfo)
                    val carry = if (outcome == Unlocker.Outcome.Unlocked && origin != null) UnlockCarry.shared.issue(origin) else null
                    val response = PasskeyEntries.response(this, begin, carry)
                    runOnUiThread {
                        val result = Intent()
                        PendingIntentHandler.setBeginGetCredentialResponse(result, response)
                        finishWith(result)
                    }
                }
            }
        }
    }

    // ---- phones without a screen lock ----

    /**
     * True (and the request ends after the message) when the phone has no screen lock to check the
     * user with. The page is told "not allowed" like a cancel, not why: it has no business knowing
     * whether the phone is locked down.
     */
    private fun refuseWithoutScreenLock(end: () -> Unit): Boolean {
        if (PasskeyPolicy.refusal(this) == null) return false
        noScreenLockNow(end)
        return true
    }

    private fun noScreenLockNow(end: () -> Unit) {
        val dialog = AlertDialog.Builder(this)
            .setTitle(PasskeyPolicy.NEEDS_SCREEN_LOCK)
            .setMessage(PasskeyPolicy.NEEDS_SCREEN_LOCK_DETAIL)
            .setPositiveButton("OK", null)
            .setNeutralButton("Open settings") { _, _ ->
                try {
                    startActivity(Intent(Settings.ACTION_SECURITY_SETTINGS))
                } catch (e: Exception) {
                    // No such page on this phone; the message already says what to do.
                }
            }
            .setOnDismissListener { end() }
            .create()
        dialog.window?.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        dialog.show()
    }

    companion object {
        const val ACTION_CREATE = "app.localpass.action.CREATE_PASSKEY"
        const val ACTION_GET = "app.localpass.action.GET_PASSKEY"
        const val ACTION_UNLOCK = "app.localpass.action.UNLOCK_FOR_PASSKEYS"
        const val EXTRA_ITEM_ID = "app.localpass.extra.ITEM_ID"

        /** One-time proof, put in the picker's entries by the unlock step, that the user was just checked (see [UnlockCarry]). */
        const val EXTRA_UNLOCK_TOKEN = "app.localpass.extra.UNLOCK_TOKEN"
    }
}
