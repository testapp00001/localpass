package app.localpass

import android.os.Handler
import android.os.Looper
import java.security.SecureRandom

/**
 * A login the user asked to save while the vault was locked, held in memory only until the unlock
 * screen has run. The password never goes into an intent, a notification, a log or a file: the
 * intent carries a random token, and this holder is the only place the password waits.
 *
 * An entry is gone when it is taken, when it is discarded (the user backed out), or after
 * [LIFETIME_MS], whichever comes first. A [CharArray] is wiped when the entry goes; the strings it
 * is turned into for the call into the engine can't be wiped, which is the limit of the platform.
 */
internal object PendingSaves {
    /** Not a data class on purpose: nothing should be able to print the password. */
    class Login(val origin: String, val username: String, val password: CharArray) {
        fun wipe() = password.fill('\u0000')
    }

    private class Entry(val login: Login, val expire: Runnable)

    private const val LIFETIME_MS = 2 * 60_000L
    private val random = SecureRandom()
    private val held = HashMap<String, Entry>()
    private val main by lazy { Handler(Looper.getMainLooper()) }

    /** Keeps [login] and returns the token that fetches it back. */
    @Synchronized
    fun put(login: Login): String {
        val token = ByteArray(16).also(random::nextBytes).joinToString("") { "%02x".format(it) }
        val expire = Runnable { discard(token) }
        held[token] = Entry(login, expire)
        main.postDelayed(expire, LIFETIME_MS)
        return token
    }

    /** Removes and returns the login, or null when it is gone. The caller wipes it after use. */
    @Synchronized
    fun take(token: String): Login? {
        val entry = held.remove(token) ?: return null
        main.removeCallbacks(entry.expire)
        return entry.login
    }

    /** Forgets the login (the user backed out, or nobody came for it in time). */
    @Synchronized
    fun discard(token: String) {
        take(token)?.wipe()
    }
}
