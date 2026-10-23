package app.localpass

import android.os.Build
import android.security.keystore.KeyPermanentlyInvalidatedException
import java.security.InvalidKeyException
import java.security.UnrecoverableKeyException
import javax.crypto.BadPaddingException

/**
 * Whether a failure to read the device secret means it is lost for good (so a new one may be made)
 * or may pass (so the old one must be kept). Deleting it by mistake would wipe the wrapping of the
 * sync keys, so anything not known to be permanent counts as temporary.
 */
internal object DeviceSecretFailure {
    /** The stored file is shorter than an IV plus a GCM tag. */
    class Damaged : Exception("device secret file is damaged")

    /** IV (12 bytes) plus the GCM tag (16 bytes) is the least a stored secret can be. */
    fun wellFormed(size: Int): Boolean = size >= 12 + 16

    /**
     * [temporary] says whether Android itself reported a passing failure (the Keystore service busy
     * or restarting); it is checked first because Android reports those as an
     * [UnrecoverableKeyException] too.
     */
    fun isPermanent(e: Throwable, temporary: (Throwable) -> Boolean = ::androidSaysTemporary): Boolean {
        if (temporary(e)) return false
        return e is Damaged ||
            e is KeyPermanentlyInvalidatedException ||
            e is UnrecoverableKeyException ||
            // A key that is not the one that wrapped the secret (the original is gone, a new one was
            // made) or a file that was changed: the tag does not verify.
            e is BadPaddingException ||
            e is InvalidKeyException
    }

    /** `android.security.KeyStoreException.isTransientFailure` (Android 13+) anywhere in the cause chain. */
    private fun androidSaysTemporary(e: Throwable): Boolean {
        if (Build.VERSION.SDK_INT < 33) return false
        var cause: Throwable? = e
        var depth = 0
        while (cause != null && depth++ < 8) {
            if (cause is android.security.KeyStoreException && cause.isTransientFailure) return true
            cause = cause.cause
        }
        return false
    }
}
