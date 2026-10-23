package app.localpass

import android.app.KeyguardManager
import android.content.Context
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL

/**
 * When LocalPass may create or use a passkey. A passkey says "the owner was present", so the phone
 * must have a way to check that: a screen lock (PIN, pattern, password) that the system's own
 * prompt can ask for, or a fingerprint or face on top of it. Without one the check would silently
 * be skipped and the passkey would claim a verification that never happened, so the request is
 * refused instead. Passwords and autofill are not affected.
 */
internal object PasskeyPolicy {
    const val NEEDS_SCREEN_LOCK = "Set a screen lock to use passkeys"

    const val NEEDS_SCREEN_LOCK_DETAIL =
        "LocalPass checks that it is you, with your fingerprint or screen lock, before it creates or uses a passkey. " +
            "This phone has no screen lock. Set a PIN, pattern, password or fingerprint in Android Settings, then try again."

    /** The reason to refuse, or null when passkeys may be used. Pure so it can be tested without a phone. */
    fun refusal(deviceSecure: Boolean, canVerify: Boolean): String? =
        if (deviceSecure && canVerify) null else NEEDS_SCREEN_LOCK

    fun refusal(context: Context): String? {
        val keyguard = context.getSystemService(KeyguardManager::class.java)
        val secure = keyguard?.isDeviceSecure == true
        val canVerify = BiometricManager.from(context).canAuthenticate(BIOMETRIC_STRONG or DEVICE_CREDENTIAL) ==
            BiometricManager.BIOMETRIC_SUCCESS
        return refusal(secure, canVerify)
    }
}
