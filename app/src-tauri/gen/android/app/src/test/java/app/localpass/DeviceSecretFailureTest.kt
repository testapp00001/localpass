package app.localpass

import java.io.IOException
import java.security.InvalidKeyException
import java.security.KeyStoreException
import java.security.ProviderException
import java.security.UnrecoverableKeyException
import javax.crypto.AEADBadTagException
import javax.crypto.BadPaddingException
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** Only a secret that can never be read again may be replaced; anything else keeps it. */
class DeviceSecretFailureTest {
    private val never: (Throwable) -> Boolean = { false }

    private fun permanent(e: Throwable) = DeviceSecretFailure.isPermanent(e, never)

    @Test
    fun aMissingOrChangedKeyOrADamagedFileIsPermanent() {
        assertTrue("a key that is not the one that wrapped it", permanent(AEADBadTagException("tag mismatch")))
        assertTrue(permanent(BadPaddingException()))
        assertTrue("key invalidated", permanent(InvalidKeyException("key permanently invalidated")))
        assertTrue(permanent(UnrecoverableKeyException("no key")))
        assertTrue(permanent(DeviceSecretFailure.Damaged()))
    }

    @Test
    fun passingFailuresKeepTheSecret() {
        assertFalse("a file that could not be read", permanent(IOException("read failed")))
        assertFalse("the Keystore service is busy or restarting", permanent(KeyStoreException("busy")))
        assertFalse(permanent(ProviderException("keystore daemon not answering")))
        assertFalse("anything unknown is not a reason to delete", permanent(IllegalStateException("?")))
        assertFalse(permanent(RuntimeException("?")))
    }

    @Test
    fun whenAndroidCallsAFailureTemporaryEvenAnUnrecoverableKeyKeepsTheSecret() {
        // Android reports a busy Keystore as an UnrecoverableKeyException whose cause says "transient".
        val busy = UnrecoverableKeyException("Failed to obtain information about key")
        assertFalse(DeviceSecretFailure.isPermanent(busy) { it === busy })
    }

    @Test
    fun aFileShorterThanAnIvAndATagIsDamaged() {
        assertFalse(DeviceSecretFailure.wellFormed(0))
        assertFalse(DeviceSecretFailure.wellFormed(27))
        assertTrue(DeviceSecretFailure.wellFormed(12 + 32 + 16))
    }
}
