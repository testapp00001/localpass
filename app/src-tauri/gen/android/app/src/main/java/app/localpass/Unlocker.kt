package app.localpass

import android.util.Base64
import android.view.ViewGroup
import android.widget.EditText
import android.widget.FrameLayout
import android.widget.Toast
import androidx.appcompat.app.AlertDialog
import androidx.appcompat.app.AppCompatActivity
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricManager.Authenticators.DEVICE_CREDENTIAL
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import java.util.concurrent.Executor
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec

/**
 * Unlocking the vault, and checking that the person holding the phone is the owner, for the
 * credential services. They show a fingerprint prompt when fingerprint unlock is on and fall back
 * to the master password.
 */
internal class Unlocker(private val activity: AppCompatActivity, private val io: Executor) {
    sealed interface Outcome {
        /** The vault was already open; the user has not been checked. */
        data object AlreadyUnlocked : Outcome

        /** The user just proved who they are (fingerprint or master password). */
        data object Unlocked : Outcome

        data object Cancelled : Outcome

        data class Failed(val message: String) : Outcome
    }

    private val main: Executor = ContextCompat.getMainExecutor(activity)

    fun unlock(onDone: (Outcome) -> Unit) {
        io.execute {
            val reply = LpProvider.call(activity, "status")
            val status = reply.obj().optString("status")
            when {
                !reply.ok -> main.execute { onDone(Outcome.Failed(reply.message)) }
                status == "unlocked" -> main.execute { onDone(Outcome.AlreadyUnlocked) }
                status == "noVault" -> main.execute { onDone(Outcome.Failed("Create or join a vault in LocalPass first.")) }
                else -> {
                    val blob = if (reply.obj().optBoolean("quickUnlock")) {
                        LpProvider.call(activity, "biometricBlob").obj().let { if (it.isNull("blob")) null else it.getString("blob") }
                    } else {
                        null
                    }
                    main.execute { if (blob != null) withFingerprint(blob, onDone) else withPassword(null, onDone) }
                }
            }
        }
    }

    private fun withFingerprint(blob: String, onDone: (Outcome) -> Unit) {
        val bytes = Base64.decode(blob, Base64.NO_WRAP)
        val cipher = try {
            Cipher.getInstance(BiometricKey.TRANSFORM).apply {
                init(Cipher.DECRYPT_MODE, BiometricKey.get(create = false), GCMParameterSpec(128, bytes, 0, 12))
            }
        } catch (e: Exception) {
            // The key is gone or changed (new fingerprints were added): the master password still works.
            withPassword("Fingerprint unlock needs to be set up again in LocalPass.", onDone)
            return
        }
        val callback = object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                val key = try {
                    val secret = result.cryptoObject?.cipher?.doFinal(bytes, 12, bytes.size - 12)
                    Base64.encodeToString(secret, Base64.NO_WRAP)
                } catch (e: Exception) {
                    withPassword(null, onDone)
                    return
                }
                io.execute {
                    val reply = LpProvider.call(activity, "unlockQuick", "key" to key)
                    main.execute { if (reply.ok) onDone(Outcome.Unlocked) else withPassword(null, onDone) }
                }
            }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                when (errorCode) {
                    BiometricPrompt.ERROR_NEGATIVE_BUTTON -> withPassword(null, onDone)
                    BiometricPrompt.ERROR_USER_CANCELED, BiometricPrompt.ERROR_CANCELED -> onDone(Outcome.Cancelled)
                    else -> withPassword(errString.toString(), onDone)
                }
            }
        }
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle("Unlock LocalPass")
            .setNegativeButtonText("Use master password")
            .setAllowedAuthenticators(BIOMETRIC_STRONG)
            .build()
        try {
            BiometricPrompt(activity, main, callback).authenticate(info, BiometricPrompt.CryptoObject(cipher))
        } catch (e: Exception) {
            withPassword(null, onDone)
        }
    }

    private fun withPassword(note: String?, onDone: (Outcome) -> Unit) {
        val input = EditText(activity).apply {
            inputType = android.text.InputType.TYPE_CLASS_TEXT or android.text.InputType.TYPE_TEXT_VARIATION_PASSWORD
            hint = "Master password"
            // The master password goes to nobody but LocalPass: no autofill service is asked about it (RD-5).
            importantForAutofill = android.view.View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
        }
        val pad = (20 * activity.resources.displayMetrics.density).toInt()
        val box = FrameLayout(activity).apply {
            setPadding(pad, pad / 2, pad, 0)
            addView(input, FrameLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT))
        }
        val dialog = AlertDialog.Builder(activity)
            .setTitle("Unlock LocalPass")
            .apply { if (note != null) setMessage(note) }
            .setView(box)
            .setPositiveButton("Unlock", null)
            .setNegativeButton("Cancel") { _, _ -> onDone(Outcome.Cancelled) }
            .setOnCancelListener { onDone(Outcome.Cancelled) }
            .create()
        // A dialog is a window of its own: the activity's FLAG_SECURE does not cover it (RD-2).
        dialog.window?.setFlags(
            android.view.WindowManager.LayoutParams.FLAG_SECURE,
            android.view.WindowManager.LayoutParams.FLAG_SECURE,
        )
        dialog.setOnShowListener {
            // Keep the dialog open on a wrong password instead of closing it.
            dialog.getButton(AlertDialog.BUTTON_POSITIVE).setOnClickListener {
                val password = input.text.toString()
                if (password.isEmpty()) return@setOnClickListener
                dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled = false
                io.execute {
                    val reply = LpProvider.call(activity, "unlockPassword", "password" to password)
                    main.execute {
                        when {
                            reply.ok -> {
                                dialog.dismiss()
                                onDone(Outcome.Unlocked)
                            }
                            reply.code == "wrongPassword" -> {
                                Toast.makeText(activity, "Wrong master password", Toast.LENGTH_SHORT).show()
                                input.text.clear()
                                dialog.getButton(AlertDialog.BUTTON_POSITIVE).isEnabled = true
                            }
                            else -> {
                                dialog.dismiss()
                                onDone(Outcome.Failed(reply.message))
                            }
                        }
                    }
                }
            }
        }
        dialog.show()
    }

    /**
     * Asks the owner to confirm with a fingerprint, face or screen lock. Reports true when the
     * owner was verified, false when the phone has nothing to verify with (no screen lock: the
     * caller must refuse, never go on as if the owner had been checked), and null when they backed
     * out. [PasskeyPolicy] is checked before a flow starts, so false is only a change of mind by the
     * phone in between.
     */
    fun verifyUser(title: String, subtitle: String, onDone: (Boolean?) -> Unit) {
        val allowed = BIOMETRIC_STRONG or DEVICE_CREDENTIAL
        if (BiometricManager.from(activity).canAuthenticate(allowed) != BiometricManager.BIOMETRIC_SUCCESS) {
            onDone(false)
            return
        }
        val callback = object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) = onDone(true)

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) = onDone(null)
        }
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(title)
            .setSubtitle(subtitle)
            .setAllowedAuthenticators(allowed)
            .build()
        try {
            BiometricPrompt(activity, main, callback).authenticate(info)
        } catch (e: Exception) {
            onDone(null)
        }
    }
}
