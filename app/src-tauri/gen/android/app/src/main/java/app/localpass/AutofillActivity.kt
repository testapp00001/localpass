package app.localpass

import android.app.assist.AssistStructure
import android.content.Intent
import android.os.Bundle
import android.service.autofill.Dataset
import android.view.WindowManager
import android.view.autofill.AutofillId
import android.view.autofill.AutofillManager
import android.view.autofill.AutofillValue
import android.widget.RemoteViews
import android.widget.Toast
import androidx.appcompat.app.AppCompatActivity
import androidx.core.content.IntentCompat
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * The steps the autofill dropdown sends the user through: unlocking the vault (so its logins can
 * be listed) and fetching the one login they chose. The result goes back to the system, which
 * fills the page. The same unlock screen also finishes saving a login that was typed while the
 * vault was locked ([ACTION_SAVE_LOGIN]).
 */
class AutofillActivity : AppCompatActivity() {
    private val io: ExecutorService = Executors.newSingleThreadExecutor()
    private lateinit var unlocker: Unlocker
    private var saveToken: String? = null

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        // Keep logins out of screenshots and the recent-apps preview.
        window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        unlocker = Unlocker(this, io)
        if (intent.action == ACTION_SAVE_LOGIN) {
            saveAfterUnlock(intent.getStringExtra(EXTRA_SAVE_TOKEN))
            return
        }
        val form = checkedForm()
        if (form == null) {
            cancel()
            return
        }
        unlocker.unlock { outcome ->
            when (outcome) {
                Unlocker.Outcome.Cancelled -> cancel()
                is Unlocker.Outcome.Failed -> {
                    Toast.makeText(this, outcome.message, Toast.LENGTH_LONG).show()
                    cancel()
                }
                else -> when (intent.action) {
                    ACTION_UNLOCK_FILL -> listLogins(form)
                    ACTION_FILL_ITEM -> fillItem(form, intent.getStringExtra(EXTRA_ITEM_ID))
                    else -> cancel()
                }
            }
        }
    }

    override fun onDestroy() {
        // Leaving without saving (backing out, or the system closing the screen) keeps nothing.
        if (isFinishing) saveToken?.let { PendingSaves.discard(it) }
        io.shutdown()
        super.onDestroy()
    }

    private fun cancel() {
        setResult(RESULT_CANCELED)
        finish()
    }

    /**
     * The form to work on, or null when the request cannot be trusted. The extras only say what the
     * autofill service meant; the system adds the real screen to the intent, and that is what is
     * believed: the app it came from must be a trusted browser (see [BrowserTrust]) and the website
     * it reports must be the one in the extras, so a request that was changed, or made up, or that
     * belongs to another app does nothing.
     */
    private fun checkedForm(): AutofillParser.Form? {
        val origin = intent.getStringExtra(EXTRA_ORIGIN) ?: return null
        val structure = IntentCompat.getParcelableExtra(intent, AutofillManager.EXTRA_ASSIST_STRUCTURE, AssistStructure::class.java) ?: return null
        if (!BrowserTrust.isTrustedPackage(this, BrowserTrust.packageOf(structure))) return null
        if (AutofillParser.originOf(structure) != origin) return null
        return AutofillParser.Form(
            origin,
            IntentCompat.getParcelableExtra(intent, EXTRA_USERNAME_ID, AutofillId::class.java),
            IntentCompat.getParcelableExtra(intent, EXTRA_PASSWORD_ID, AutofillId::class.java),
        )
    }

    /**
     * A login the user chose to save while the vault was locked: unlock first (the fingerprint, or
     * the master password), then store it. Backing out of the unlock saves nothing.
     */
    private fun saveAfterUnlock(token: String?) {
        if (token == null) return cancel()
        saveToken = token
        unlocker.unlock { outcome ->
            when (outcome) {
                Unlocker.Outcome.Cancelled -> {
                    PendingSaves.discard(token)
                    SaveLogins.toast(this, "Not saved: LocalPass stayed locked")
                    cancel()
                }
                is Unlocker.Outcome.Failed -> {
                    PendingSaves.discard(token)
                    SaveLogins.toast(this, outcome.message)
                    cancel()
                }
                else -> storeLogin(token)
            }
        }
    }

    private fun storeLogin(token: String) {
        val login = PendingSaves.take(token)
        if (login == null) {
            SaveLogins.toast(this, "Not saved: the request timed out. Sign in again to save it.")
            return cancel()
        }
        io.execute {
            val reply = try {
                SaveLogins.store(this, login)
            } finally {
                login.wipe()
            }
            runOnUiThread {
                SaveLogins.toast(this, SaveLogins.message(reply))
                cancel()
            }
        }
    }

    private fun answer(result: android.os.Parcelable?) {
        if (result == null) {
            Toast.makeText(this, "No saved login for this website", Toast.LENGTH_SHORT).show()
            return cancel()
        }
        setResult(RESULT_OK, Intent().putExtra(AutofillManager.EXTRA_AUTHENTICATION_RESULT, result))
        finish()
    }

    /** After unlocking: the logins for the site, still one tap away from being filled. */
    private fun listLogins(form: AutofillParser.Form) {
        io.execute {
            val response = AutofillResponses.logins(this, form)
            runOnUiThread { answer(response) }
        }
    }

    /** The chosen login's username and password, as the values for the page's fields. */
    private fun fillItem(form: AutofillParser.Form, itemId: String?) {
        if (itemId == null) return cancel()
        io.execute {
            val fill = LpProvider.call(this, "fillLogin", "origin" to form.origin, "itemId" to itemId)
            runOnUiThread {
                if (!fill.ok) {
                    Toast.makeText(this, fill.message, Toast.LENGTH_LONG).show()
                    return@runOnUiThread cancel()
                }
                val login = fill.obj()
                val view = RemoteViews(packageName, R.layout.autofill_item)
                val dataset = Dataset.Builder(view)
                form.username?.let { dataset.setValue(it, AutofillValue.forText(login.optString("username"))) }
                form.password?.let { dataset.setValue(it, AutofillValue.forText(login.optString("password"))) }
                answer(dataset.build())
            }
        }
    }

    companion object {
        const val ACTION_UNLOCK_FILL = "app.localpass.action.UNLOCK_FOR_AUTOFILL"
        const val ACTION_FILL_ITEM = "app.localpass.action.FILL_ITEM"
        const val ACTION_SAVE_LOGIN = "app.localpass.action.SAVE_LOGIN"
        const val EXTRA_SAVE_TOKEN = "app.localpass.extra.SAVE_TOKEN"
        const val EXTRA_ORIGIN = "app.localpass.extra.ORIGIN"
        const val EXTRA_ITEM_ID = "app.localpass.extra.ITEM_ID"
        const val EXTRA_USERNAME_ID = "app.localpass.extra.USERNAME_ID"
        const val EXTRA_PASSWORD_ID = "app.localpass.extra.PASSWORD_ID"
    }
}
