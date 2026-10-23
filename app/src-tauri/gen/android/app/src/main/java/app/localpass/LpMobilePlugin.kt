package app.localpass

import android.app.Activity
import android.content.ClipData
import android.content.ClipDescription
import android.content.ClipboardManager
import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import android.view.autofill.AutofillManager
import android.os.Handler
import android.os.Looper
import android.os.PersistableBundle
import android.security.keystore.KeyPermanentlyInvalidatedException
import android.util.Base64
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricManager.Authenticators.BIOMETRIC_STRONG
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat
import androidx.fragment.app.FragmentActivity
import app.tauri.annotation.Command
import app.tauri.annotation.InvokeArg
import app.tauri.annotation.TauriPlugin
import app.tauri.plugin.Invoke
import app.tauri.plugin.JSObject
import app.tauri.plugin.Plugin
import javax.crypto.Cipher
import javax.crypto.spec.GCMParameterSpec

@InvokeArg
class CopyArgs {
    var text: String = ""
    var clearAfterMs: Long = 30_000
}

@InvokeArg
class SecretArgs {
    var secret: String = ""
}

@InvokeArg
class BlobArgs {
    var blob: String = ""
}

@InvokeArg
class TargetArgs {
    var target: String = ""
}

/** Android-only helpers called from Rust (src-tauri/src/mobile.rs). */
@TauriPlugin
class LpMobilePlugin(private val activity: Activity) : Plugin(activity) {
    private val main = Handler(Looper.getMainLooper())
    private var clearTask: Runnable? = null

    private companion object {
        const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
    }

    /** Copies a secret, hides it from clipboard previews, and clears it later. */
    @Command
    fun copySensitive(invoke: Invoke) {
        val args = invoke.parseArgs(CopyArgs::class.java)
        // Rust calls this from a worker thread; clipboard and timers belong on the UI thread.
        activity.runOnUiThread {
            try {
                val cm = activity.getSystemService(Context.CLIPBOARD_SERVICE) as ClipboardManager
                val clip = ClipData.newPlainText("LocalPass", args.text)
                clip.description.extras = PersistableBundle().apply {
                    val key = if (Build.VERSION.SDK_INT >= 33) ClipDescription.EXTRA_IS_SENSITIVE else "android.content.extra.IS_SENSITIVE"
                    putBoolean(key, true)
                }
                cm.setPrimaryClip(clip)
                clearTask?.let { main.removeCallbacks(it) }
                if (args.clearAfterMs > 0) {
                    val task = Runnable {
                        if (Build.VERSION.SDK_INT >= 28) cm.clearPrimaryClip() else cm.setPrimaryClip(ClipData.newPlainText("", ""))
                    }
                    clearTask = task
                    main.postDelayed(task, args.clearAfterMs)
                }
                invoke.resolve()
            } catch (e: Exception) {
                invoke.reject(e.message ?: "Could not copy")
            }
        }
    }

    /**
     * Where the phone-level set-up stands, for Settings > Autofill and passkeys. Each part is one of
     * "unsupported" (this Android has no such feature), "on", "off", or "unknown" (Android does not
     * let an app read it, so the screen must not claim either).
     */
    @Command
    fun setupStatus(invoke: Invoke) {
        invoke.resolve(
            JSObject()
                .put("sdk", Build.VERSION.SDK_INT)
                .put("autofill", autofillState())
                .put("credentialProvider", credentialProviderState())
                .put("screenLock", screenLockState())
                .put("localNetwork", localNetworkState()),
        )
    }

    private fun autofillState(): String {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) return "unsupported"
        val manager = activity.getSystemService(AutofillManager::class.java)
        if (manager == null || !manager.isAutofillSupported) return "unsupported"
        // True only when this app is the selected autofill service.
        return if (manager.hasEnabledAutofillServices()) "on" else "off"
    }

    /**
     * Whether LocalPass is switched on as a passkey provider cannot be read: the setting that holds
     * it (`credential_service`) is closed to apps ("is not readable ... restricted to system apps",
     * checked on Android 17), and the check that exists for it is system-only. So this is never "on"
     * or "off", and the screen sends the user to the system page instead of guessing.
     */
    private fun credentialProviderState(): String =
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) "unsupported" else "unknown"

    /** "off" when the phone has no screen lock to check the owner with: LocalPass refuses passkeys then (see [PasskeyPolicy]). */
    private fun screenLockState(): String =
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.UPSIDE_DOWN_CAKE) "unsupported" else if (PasskeyPolicy.refusal(activity) == null) "on" else "off"

    /** Android 17+ blocks talking to other devices on the home network until the user allows it. */
    private fun localNetworkState(): String {
        if (Build.VERSION.SDK_INT < 37) return "unsupported"
        return if (activity.checkSelfPermission(LOCAL_NETWORK) == PackageManager.PERMISSION_GRANTED) "on" else "off"
    }

    /** Opens the system page for one set-up step: "autofill", "credentials", "app" (permissions) or "security" (screen lock). */
    @Command
    fun openSetup(invoke: Invoke) {
        val target = invoke.parseArgs(TargetArgs::class.java).target
        val uri = Uri.parse("package:${activity.packageName}")
        // The first intent is the most direct page; the later ones are fallbacks if this phone lacks it.
        val intents = when (target) {
            "autofill" -> listOf(
                Intent(Settings.ACTION_REQUEST_SET_AUTOFILL_SERVICE, uri),
                Intent(Settings.ACTION_SETTINGS),
            )
            "credentials" -> if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
                listOf(Intent(Settings.ACTION_CREDENTIAL_PROVIDER, uri), Intent(Settings.ACTION_CREDENTIAL_PROVIDER), Intent(Settings.ACTION_SETTINGS))
            } else {
                emptyList()
            }
            "app" -> listOf(Intent(Settings.ACTION_APPLICATION_DETAILS_SETTINGS, uri))
            "security" -> listOf(Intent(Settings.ACTION_SECURITY_SETTINGS), Intent(Settings.ACTION_SETTINGS))
            else -> emptyList()
        }
        if (intents.isEmpty()) {
            invoke.reject("This is not available on this version of Android")
            return
        }
        // Rust calls this from a worker thread; screens are started from the UI thread.
        activity.runOnUiThread {
            val opened = intents.any { intent ->
                try {
                    activity.startActivity(intent)
                    true
                } catch (e: Exception) {
                    false
                }
            }
            if (opened) invoke.resolve() else invoke.reject("Android could not open its settings")
        }
    }

    @Command
    fun biometricStatus(invoke: Invoke) {
        val ok = BiometricManager.from(activity).canAuthenticate(BIOMETRIC_STRONG) == BiometricManager.BIOMETRIC_SUCCESS
        invoke.resolve(JSObject().put("available", ok))
    }

    /** Encrypts the quick-unlock key with a new biometric-bound Keystore key. */
    @Command
    fun biometricEnroll(invoke: Invoke) {
        val secret = Base64.decode(invoke.parseArgs(SecretArgs::class.java).secret, Base64.NO_WRAP)
        val cipher = try {
            Cipher.getInstance(BiometricKey.TRANSFORM).apply { init(Cipher.ENCRYPT_MODE, BiometricKey.get(create = true)) }
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Keystore error")
            return
        }
        prompt(cipher, invoke, "Turn on fingerprint unlock") { c ->
            JSObject().put("blob", Base64.encodeToString(c.iv + c.doFinal(secret), Base64.NO_WRAP))
        }
    }

    /** Decrypts the quick-unlock key after a biometric check. */
    @Command
    fun biometricDecrypt(invoke: Invoke) {
        val blob = Base64.decode(invoke.parseArgs(BlobArgs::class.java).blob, Base64.NO_WRAP)
        val cipher = try {
            Cipher.getInstance(BiometricKey.TRANSFORM).apply {
                init(Cipher.DECRYPT_MODE, BiometricKey.get(create = false), GCMParameterSpec(128, blob, 0, 12))
            }
        } catch (e: KeyPermanentlyInvalidatedException) {
            invoke.reject("Your fingerprints or face changed. Unlock with your master password, then turn biometric unlock on again.")
            return
        } catch (e: Exception) {
            invoke.reject(e.message ?: "Keystore error")
            return
        }
        prompt(cipher, invoke, "Unlock LocalPass") { c ->
            JSObject().put("secret", Base64.encodeToString(c.doFinal(blob, 12, blob.size - 12), Base64.NO_WRAP))
        }
    }

    private fun prompt(cipher: Cipher, invoke: Invoke, title: String, onOk: (Cipher) -> JSObject) {
        val fa = activity as? FragmentActivity ?: run {
            invoke.reject("Biometrics are not available")
            return
        }
        val callback = object : BiometricPrompt.AuthenticationCallback() {
            override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
                val c = result.cryptoObject?.cipher ?: run {
                    invoke.reject("Biometric check returned no key")
                    return
                }
                try {
                    invoke.resolve(onOk(c))
                } catch (e: Exception) {
                    invoke.reject(e.message ?: "Crypto error")
                }
            }

            override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
                // Backing out isn't an error; the app just shows the password field.
                val backedOut = errorCode == BiometricPrompt.ERROR_USER_CANCELED ||
                    errorCode == BiometricPrompt.ERROR_NEGATIVE_BUTTON ||
                    errorCode == BiometricPrompt.ERROR_CANCELED
                invoke.reject(if (backedOut) "cancelled" else errString.toString())
            }
        }
        val info = BiometricPrompt.PromptInfo.Builder()
            .setTitle(title)
            .setNegativeButtonText("Use master password")
            .setAllowedAuthenticators(BIOMETRIC_STRONG)
            .build()
        // Rust calls this from a worker thread; BiometricPrompt must be shown on the UI thread.
        activity.runOnUiThread {
            try {
                BiometricPrompt(fa, ContextCompat.getMainExecutor(activity), callback)
                    .authenticate(info, BiometricPrompt.CryptoObject(cipher))
            } catch (e: Exception) {
                invoke.reject(e.message ?: "Biometric prompt failed")
            }
        }
    }
}
