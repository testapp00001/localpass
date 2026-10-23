package app.localpass

/** JNI entry points implemented in Rust (src-tauri/src/android.rs). */
object NativeBridge {
    init {
        System.loadLibrary("localpass_lib")
    }

    /** Gives Rust its data directory and the device secret. Call before the Tauri activity starts. */
    @JvmStatic external fun init(dataDir: String, deviceSecret: ByteArray)

    /** Runs one discovery + sync round with nearby devices. Returns a short status line. */
    @JvmStatic external fun runSyncOnce(dataDir: String, deviceSecret: ByteArray, timeoutMs: Long): String

    /**
     * One JSON request for the credential services (passkeys and autofill); see
     * `lp_engine::provider` for the requests. Returns `{"ok": ...}` or `{"error": {...}}`.
     */
    @JvmStatic external fun credentialCall(dataDir: String, deviceSecret: ByteArray, request: String): String

    @JvmStatic external fun onAppBackground()

    @JvmStatic external fun onAppForeground()
}
