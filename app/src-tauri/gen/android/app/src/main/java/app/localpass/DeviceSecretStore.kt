package app.localpass

import android.content.Context
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import android.util.Log
import java.io.File
import java.security.KeyStore
import java.security.SecureRandom
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/**
 * The device secret protects this device's sync keys so background sync can
 * run while the vault is locked. It never decrypts vault data. It is random
 * and stored encrypted by a non-exportable Android Keystore key.
 */
object DeviceSecretStore {
    private const val ALIAS = "localpass_device_secret"
    private const val FILE = "device.secret"
    private const val TRANSFORM = "AES/GCM/NoPadding"
    private const val ATTEMPTS = 3

    /** The Keystore could not be used right now. The stored secret is untouched; try again later. */
    class Unavailable(cause: Throwable) :
        IllegalStateException("Android's key store is not answering. Close LocalPass and open it again in a moment.", cause)

    @Synchronized
    fun get(context: Context): ByteArray {
        val file = File(context.filesDir, FILE)
        if (file.exists()) {
            try {
                return readWithRetry(file)
            } catch (e: Exception) {
                if (!DeviceSecretFailure.isPermanent(e)) throw Unavailable(e)
                // The Keystore key is gone or changed, or the file is damaged (e.g. data restored onto
                // another phone): the secret can never be read again, so start over. The engine
                // re-creates its sync keys on the next unlock. Only this case may delete the file.
                Log.w("LocalPass", "device secret can no longer be read; creating a new one", e)
                file.delete()
            }
        }
        val secret = ByteArray(32).also { SecureRandom().nextBytes(it) }
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.ENCRYPT_MODE, keystoreKey())
        val tmp = File(context.filesDir, "$FILE.tmp")
        tmp.writeBytes(cipher.iv + cipher.doFinal(secret))
        if (!tmp.renameTo(file)) throw IllegalStateException("could not store device secret")
        return secret
    }

    /** A busy or restarting Keystore usually answers a moment later; only a lasting failure is reported. */
    private fun readWithRetry(file: File): ByteArray {
        var failure: Exception? = null
        for (attempt in 1..ATTEMPTS) {
            try {
                return read(file)
            } catch (e: Exception) {
                if (DeviceSecretFailure.isPermanent(e)) throw e
                failure = e
                Log.w("LocalPass", "device secret not readable yet (attempt $attempt of $ATTEMPTS)", e)
                if (attempt < ATTEMPTS) Thread.sleep(150L * attempt)
            }
        }
        throw failure!!
    }

    private fun read(file: File): ByteArray {
        val blob = file.readBytes()
        if (!DeviceSecretFailure.wellFormed(blob.size)) throw DeviceSecretFailure.Damaged()
        val cipher = Cipher.getInstance(TRANSFORM)
        cipher.init(Cipher.DECRYPT_MODE, keystoreKey(), GCMParameterSpec(128, blob, 0, 12))
        return cipher.doFinal(blob, 12, blob.size - 12)
    }

    private fun keystoreKey(): SecretKey {
        val ks = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (ks.getKey(ALIAS, null) as? SecretKey)?.let { return it }
        val gen = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        gen.init(
            KeyGenParameterSpec.Builder(ALIAS, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
        )
        return gen.generateKey()
    }
}
