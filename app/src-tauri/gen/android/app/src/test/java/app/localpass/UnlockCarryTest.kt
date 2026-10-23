package app.localpass

import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/** An unlock vouches for one passkey pick, once, for its own website, and briefly. */
class UnlockCarryTest {
    private var now = 1_000L
    private var counter = 0
    private val carry = UnlockCarry(clock = { now }, newToken = { "token-${counter++}" })
    private val site = "https://example.com"

    @Test
    fun aTokenCarriesOnceForTheWebsiteItWasIssuedFor() {
        val token = carry.issue(site)
        assertTrue(carry.redeem(token, site))
        assertFalse("a second use finds nothing", carry.redeem(token, site))
    }

    @Test
    fun anotherWebsiteGetsNothingAndUsesUpTheToken() {
        val token = carry.issue(site)
        assertFalse(carry.redeem(token, "https://evil.test"))
        assertFalse("the right site cannot use it afterwards", carry.redeem(token, site))
    }

    @Test
    fun aTokenDiesAfterThirtySeconds() {
        val token = carry.issue(site)
        now += UnlockCarry.LIFETIME_MS - 1
        assertTrue(carry.redeem(token, site))
        val late = carry.issue(site)
        now += UnlockCarry.LIFETIME_MS
        assertFalse(carry.redeem(late, site))
    }

    @Test
    fun noTokenAndUnknownTokensNeverCarry() {
        carry.issue(site)
        assertFalse(carry.redeem(null, site))
        assertFalse(carry.redeem("", site))
        assertFalse(carry.redeem("guess", site))
    }

    @Test
    fun eachUnlockGetsItsOwnToken() {
        val first = carry.issue(site)
        val second = carry.issue(site)
        assertNotEquals(first, second)
        assertTrue(carry.redeem(second, site))
        assertTrue("using one does not use the other", carry.redeem(first, site))
    }

    @Test
    fun onlyAFewTokensWaitAtOnce() {
        val oldest = carry.issue(site)
        repeat(UnlockCarry.MAX_OUTSTANDING) { carry.issue(site) }
        assertFalse(carry.redeem(oldest, site))
    }

    @Test
    fun nothingCarriesUntilSomeoneIssuedAToken() {
        // The autofill and save paths only use Unlocker, which has no connection to this class: the
        // only holder is the shared instance, and its only way in is issue().
        assertFalse(UnlockCarry.shared.redeem("00000000000000000000000000000000", site))
    }
}
