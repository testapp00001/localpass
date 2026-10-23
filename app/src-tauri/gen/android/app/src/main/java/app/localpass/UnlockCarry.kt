package app.localpass

import java.security.SecureRandom

/**
 * Lets the fingerprint (or master password) that unlocked the vault for the passkey picker also
 * count as the user check for the one passkey the user then picks, so there is one prompt instead
 * of two.
 *
 * It is a token, not a timestamp: [issue] is called only by the picker's own unlock step
 * (`CredentialActivity` with the unlock action), for the website that step was for, and the token
 * travels only inside the entries that step returns to the system. An unlock anywhere else
 * (autofill, saving a login, the app itself) never issues one, so it can never vouch for a passkey.
 * A token works once, only for the website it was issued for, and only for [LIFETIME_MS].
 */
internal class UnlockCarry(
    private val clock: () -> Long = { System.nanoTime() / 1_000_000 },
    private val newToken: () -> String = ::randomToken,
) {
    private class Issued(val origin: String, val at: Long)

    private val issued = LinkedHashMap<String, Issued>()

    /** A token for the website the picker was opened for. */
    @Synchronized
    fun issue(origin: String): String {
        val now = clock()
        issued.entries.removeIf { now - it.value.at >= LIFETIME_MS }
        while (issued.size >= MAX_OUTSTANDING) issued.remove(issued.keys.first())
        val token = newToken()
        issued[token] = Issued(origin, now)
        return token
    }

    /**
     * True once for a fresh token that was issued for [origin]. The token is used up whether or not
     * it fits, so a wrong guess or a replay leaves nothing to try again.
     */
    @Synchronized
    fun redeem(token: String?, origin: String): Boolean {
        if (token.isNullOrEmpty()) return false
        val entry = issued.remove(token) ?: return false
        return entry.origin == origin && clock() - entry.at < LIFETIME_MS
    }

    companion object {
        const val LIFETIME_MS = 30_000L
        const val MAX_OUTSTANDING = 8

        /** The one holder the credential screens share (they run in one process). */
        val shared = UnlockCarry()

        private val random = SecureRandom()

        private fun randomToken(): String = ByteArray(16).also(random::nextBytes).joinToString("") { "%02x".format(it) }
    }
}
