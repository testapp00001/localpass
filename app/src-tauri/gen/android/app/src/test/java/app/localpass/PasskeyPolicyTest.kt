package app.localpass

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Passkeys are refused on a phone that cannot check the owner (no screen lock). */
class PasskeyPolicyTest {
    @Test
    fun aPhoneWithAScreenLockAndAWayToCheckIsAllowed() {
        assertNull(PasskeyPolicy.refusal(deviceSecure = true, canVerify = true))
    }

    @Test
    fun aPhoneWithoutAScreenLockIsRefused() {
        assertEquals("Set a screen lock to use passkeys", PasskeyPolicy.refusal(deviceSecure = false, canVerify = false))
        assertNotNull(PasskeyPolicy.refusal(deviceSecure = false, canVerify = true))
    }

    @Test
    fun aScreenLockThatCannotAnswerThePromptIsRefusedToo() {
        assertNotNull(PasskeyPolicy.refusal(deviceSecure = true, canVerify = false))
    }

    @Test
    fun theLongerMessageSaysWhatToDo() {
        assertTrue("screen lock" in PasskeyPolicy.NEEDS_SCREEN_LOCK_DETAIL && "try again" in PasskeyPolicy.NEEDS_SCREEN_LOCK_DETAIL)
    }
}
