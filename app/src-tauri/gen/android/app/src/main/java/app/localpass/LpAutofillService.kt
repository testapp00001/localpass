package app.localpass

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import android.os.CancellationSignal
import android.os.Handler
import android.os.Looper
import android.service.autofill.AutofillService
import android.service.autofill.Dataset
import android.service.autofill.FillCallback
import android.service.autofill.FillRequest
import android.service.autofill.FillResponse
import android.service.autofill.SaveCallback
import android.service.autofill.SaveInfo
import android.service.autofill.SaveRequest
import android.widget.RemoteViews
import android.widget.Toast
import java.util.concurrent.ExecutorService
import java.util.concurrent.Executors

/**
 * LocalPass as an autofill service: logins for the website in the browser in front. It acts only
 * for a browser on Google's trusted list ([BrowserTrust]); any other app, even one showing a web
 * view that claims to be a site, gets nothing.
 *
 * Choosing a login (and unlocking, when the vault is locked) opens [AutofillActivity], so no
 * password is handed to the system until the user has picked that one login. A login typed into a
 * form LocalPass has no entry for (or has with another password) is offered to Android's save
 * prompt; see [onSaveRequest].
 */
class LpAutofillService : AutofillService() {
    private val io: ExecutorService = Executors.newSingleThreadExecutor()

    override fun onDestroy() {
        io.shutdown()
        super.onDestroy()
    }

    override fun onFillRequest(request: FillRequest, cancellationSignal: CancellationSignal, callback: FillCallback) {
        val structure = request.fillContexts.lastOrNull()?.structure
        val browser = BrowserTrust.packageOf(structure)
        val form = structure?.let { AutofillParser.parse(it) }
        if (form == null) {
            callback.onSuccess(null)
            return
        }
        io.execute {
            try {
                // The website in the form is only the browser's word for it: believe a browser on
                // the trusted list, and nothing else (no logins, no save offer, not even unlock).
                callback.onSuccess(if (BrowserTrust.isTrustedPackage(this, browser)) AutofillResponses.fill(this, form) else null)
            } catch (e: Exception) {
                callback.onFailure(e.message)
            }
        }
    }

    /**
     * The user tapped Save on the system's prompt (that tap is the confirmation; backing out of the
     * prompt never gets here). The username and password come from the saved screen, and the
     * website from the same place the fill path takes it from.
     */
    override fun onSaveRequest(request: SaveRequest, callback: SaveCallback) {
        val browsers = request.fillContexts.map { BrowserTrust.packageOf(it.structure) }
        io.execute {
            // Same rule as filling: only a trusted browser's word for the website is believed.
            if (browsers.isEmpty() || !browsers.all { BrowserTrust.isTrustedPackage(this, it) }) {
                callback.onSuccess()
                return@execute
            }
            // The newest screen state first: it holds the values as they were when the form was sent.
            val typed = request.fillContexts.asReversed().firstNotNullOfOrNull { AutofillParser.typed(it.structure) }
            if (typed == null) {
                callback.onSuccess()
                return@execute
            }
            val login = PendingSaves.Login(typed.origin, typed.username, typed.password)
            try {
                SaveLogins.save(this, login, callback)
            } catch (e: Exception) {
                login.wipe()
                callback.onFailure("LocalPass could not save this login.")
            }
        }
    }
}

/** Saving a login the user typed into a website: now when the vault is open, else after an unlock. */
internal object SaveLogins {
    fun toast(context: Context, text: String) {
        val app = context.applicationContext
        Handler(Looper.getMainLooper()).post { Toast.makeText(app, text, Toast.LENGTH_LONG).show() }
    }

    /** Stores the login in the vault (the engine adds it, updates its password, or finds it unchanged). */
    fun store(context: Context, login: PendingSaves.Login): LpProvider.Reply =
        LpProvider.call(
            context,
            "saveLogin",
            "origin" to login.origin,
            "username" to login.username,
            "password" to String(login.password),
        )

    /** What to tell the user once [store] has answered. */
    fun message(reply: LpProvider.Reply): String =
        if (reply.ok) {
            when (reply.obj().optString("status")) {
                "created" -> "Saved to LocalPass"
                "updated" -> "Password updated in LocalPass"
                else -> "Already saved in LocalPass"
            }
        } else {
            "LocalPass could not save this login: ${reply.message}"
        }

    /** Blocking; call from a background thread. Takes over (and wipes) [login]. */
    fun save(context: Context, login: PendingSaves.Login, callback: SaveCallback) {
        val reply = store(context, login)
        if (reply.ok || reply.code != "locked") {
            login.wipe()
            toast(context, message(reply))
            callback.onSuccess()
            return
        }
        // Locked: nothing is saved until the user has unlocked, so ask for that on a screen of ours.
        // The password waits in memory under a token; the intent carries only the token.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            val token = PendingSaves.put(login)
            val sender = try {
                PendingIntent.getActivity(
                    context,
                    SAVE_REQUEST_CODE,
                    Intent(context, AutofillActivity::class.java)
                        .setAction(AutofillActivity.ACTION_SAVE_LOGIN)
                        .putExtra(AutofillActivity.EXTRA_SAVE_TOKEN, token),
                    PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_CANCEL_CURRENT,
                ).intentSender
            } catch (e: Exception) {
                PendingSaves.discard(token)
                null
            }
            if (sender != null) {
                callback.onSuccess(sender)
                return
            }
        } else {
            login.wipe()
        }
        // Older Android can't launch a screen from here: say so instead of losing the login quietly.
        toast(context, "LocalPass is locked, so this password was not saved. Unlock LocalPass and sign in again to save it.")
        callback.onSuccess()
    }

    private const val SAVE_REQUEST_CODE = 2
}

/** Builds what the autofill dropdown shows. */
internal object AutofillResponses {
    private fun row(context: Context, title: String, subtitle: String): RemoteViews =
        RemoteViews(context.packageName, R.layout.autofill_item).apply {
            setTextViewText(R.id.title, title)
            setTextViewText(R.id.subtitle, subtitle)
        }

    private fun pending(context: Context, code: Int, intent: Intent) =
        // The system adds the screen and client state to this intent, so it can't be immutable.
        PendingIntent.getActivity(context, code, intent, PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_CANCEL_CURRENT).intentSender

    private fun intent(context: Context, action: String, form: AutofillParser.Form) =
        Intent(context, AutofillActivity::class.java).setAction(action)
            .putExtra(AutofillActivity.EXTRA_ORIGIN, form.origin)
            .putExtra(AutofillActivity.EXTRA_USERNAME_ID, form.username)
            .putExtra(AutofillActivity.EXTRA_PASSWORD_ID, form.password)

    /**
     * Asks Android to offer saving what the user types into a login form (the user still has to
     * tap Save). Only for a form with a username and a password field, so search boxes and the
     * single fields of a multi-step login are left alone. The save prompt appears after the form
     * is submitted, or, for a page that never reloads, once its fields disappear.
     */
    private fun saveInfo(form: AutofillParser.Form): SaveInfo? {
        val username = form.username ?: return null
        val password = form.password ?: return null
        return SaveInfo.Builder(SaveInfo.SAVE_DATA_TYPE_USERNAME or SaveInfo.SAVE_DATA_TYPE_PASSWORD, arrayOf(password))
            .setOptionalIds(arrayOf(username))
            .setFlags(SaveInfo.FLAG_SAVE_ON_ALL_VIEWS_INVISIBLE)
            .build()
    }

    /** The dropdown for a form: an unlock prompt when the vault is locked, else the matching logins. */
    fun fill(context: Context, form: AutofillParser.Form): FillResponse? {
        val status = LpProvider.call(context, "status")
        if (!status.ok) return null
        return when (status.obj().optString("status")) {
            "locked" -> FillResponse.Builder()
                .setAuthentication(
                    form.ids,
                    pending(context, 1, intent(context, AutofillActivity.ACTION_UNLOCK_FILL, form)),
                    row(context, "LocalPass", "Tap to unlock"),
                )
                .apply { saveInfo(form)?.let { setSaveInfo(it) } }
                .build()
            // No saved login for the site is still worth a response when the form can be saved.
            "unlocked" -> logins(context, form) ?: saveInfo(form)?.let { FillResponse.Builder().setSaveInfo(it).build() }
            else -> null
        }
    }

    /**
     * One entry per saved login for the site, or null when there is none. Choosing one runs
     * [AutofillActivity] to fetch it. The response replaces the one it answers, so it carries the
     * save offer again.
     */
    fun logins(context: Context, form: AutofillParser.Form): FillResponse? {
        val found = LpProvider.call(context, "findLogins", "origin" to form.origin)
        if (!found.ok) return null
        val matches = found.array()
        if (matches.length() == 0) return null
        val response = FillResponse.Builder()
        saveInfo(form)?.let { response.setSaveInfo(it) }
        for (i in 0 until matches.length()) {
            val login = matches.getJSONObject(i)
            val id = login.getString("id")
            val view = row(context, login.optString("title"), login.optString("username").ifBlank { "(no username)" })
            val dataset = Dataset.Builder(view)
            // No values yet: the dataset needs authentication, which is where the login is fetched.
            for (field in form.ids) dataset.setValue(field, null)
            dataset.setAuthentication(
                pending(
                    context,
                    id.hashCode() and 0x7fffffff,
                    intent(context, AutofillActivity.ACTION_FILL_ITEM, form).putExtra(AutofillActivity.EXTRA_ITEM_ID, id),
                ),
            )
            response.addDataset(dataset.build())
        }
        return response.build()
    }
}
