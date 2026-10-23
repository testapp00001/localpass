package app.localpass

import android.content.pm.PackageManager
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.view.WindowManager
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import androidx.lifecycle.DefaultLifecycleObserver
import androidx.lifecycle.LifecycleOwner
import androidx.lifecycle.ProcessLifecycleOwner

class MainActivity : TauriActivity() {
  private var multicastLock: WifiManager.MulticastLock? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    // Keep vault contents out of screenshots and the recent-apps preview.
    window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
    // Rust needs its data directory and device secret before Tauri starts.
    NativeBridge.init(filesDir.absolutePath, DeviceSecretStore.get(this))
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // Edge-to-edge: keep the page clear of the status/navigation bars and above the keyboard.
    ViewCompat.setOnApplyWindowInsetsListener(findViewById(android.R.id.content)) { view, insets ->
      val bars = insets.getInsets(
        WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout() or WindowInsetsCompat.Type.ime(),
      )
      view.setPadding(bars.left, bars.top, bars.right, bars.bottom)
      WindowInsetsCompat.CONSUMED
    }
    // LocalPass's own screens (the unlock screen takes the master password) are not offered to any
    // autofill service, which could otherwise offer to save the master password elsewhere.
    findViewById<android.view.View>(android.R.id.content).importantForAutofill =
      android.view.View.IMPORTANT_FOR_AUTOFILL_NO_EXCLUDE_DESCENDANTS
    requestLocalNetworkAccess()
    SyncWorker.schedulePeriodic(this)
    ProcessLifecycleOwner.get().lifecycle.addObserver(object : DefaultLifecycleObserver {
      override fun onStart(owner: LifecycleOwner) = NativeBridge.onAppForeground()

      override fun onStop(owner: LifecycleOwner) {
        NativeBridge.onAppBackground()
        SyncWorker.scheduleSoon(applicationContext)
      }
    })
  }

  override fun onResume() {
    super.onResume()
    // mDNS discovery needs multicast, which Android filters unless a lock is held.
    val wifi = applicationContext.getSystemService(WIFI_SERVICE) as WifiManager
    multicastLock = wifi.createMulticastLock("localpass").apply {
      setReferenceCounted(false)
      acquire()
    }
  }

  override fun onPause() {
    multicastLock?.release()
    multicastLock = null
    super.onPause()
  }

  /** Android 17+ blocks local-network traffic for apps targeting it until the user allows it. */
  private fun requestLocalNetworkAccess() {
    if (Build.VERSION.SDK_INT >= 37 && checkSelfPermission(LOCAL_NETWORK) != PackageManager.PERMISSION_GRANTED) {
      requestPermissions(arrayOf(LOCAL_NETWORK), LOCAL_NETWORK_REQUEST)
    }
  }

  companion object {
    private const val LOCAL_NETWORK = "android.permission.ACCESS_LOCAL_NETWORK"
    private const val LOCAL_NETWORK_REQUEST = 4207
  }
}
