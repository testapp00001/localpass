package app.localpass

import android.os.Build
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import java.security.KeyStore
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey

/**
 * The Keystore key behind fingerprint unlock. It only works right after a biometric check, and
 * it wraps the vault's quick-unlock key. The app UI and the credential services both use it.
 */
internal object BiometricKey {
    const val TRANSFORM = "AES/GCM/NoPadding"
    private const val ALIAS = "localpass_quick_unlock"

    /** The key; `create` replaces any old one (used when turning fingerprint unlock on). */
    fun get(create: Boolean): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        if (create) ks.deleteEntry(ALIAS)
        (ks.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        if (!create) throw IllegalStateException("Biometric unlock is not set up")
        val spec = KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
            .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
            .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
            .setKeySize(256)
            .setUserAuthenticationRequired(true)
            .setInvalidatedByBiometricEnrollment(true)
        if (Build.VERSION.SDK_INT >= 30) spec.setUserAuthenticationParameters(0, KeyProperties.AUTH_BIOMETRIC_STRONG)
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(spec.build())
        return gen.generateKey()
    }
}
