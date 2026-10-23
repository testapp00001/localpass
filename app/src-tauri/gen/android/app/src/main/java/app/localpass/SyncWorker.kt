package app.localpass

import android.content.Context
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.util.Log
import androidx.work.CoroutineWorker
import androidx.work.ExistingPeriodicWorkPolicy
import androidx.work.ExistingWorkPolicy
import androidx.work.OneTimeWorkRequestBuilder
import androidx.work.PeriodicWorkRequestBuilder
import androidx.work.WorkManager
import androidx.work.WorkerParameters
import java.util.concurrent.TimeUnit
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Background sync while the app isn't open: finds LocalPass devices on the
 * current network and exchanges changes. Runs with the vault locked; only
 * encrypted data moves.
 */
class SyncWorker(context: Context, params: WorkerParameters) : CoroutineWorker(context, params) {
    override suspend fun doWork(): Result = withContext(Dispatchers.IO) {
        if (!onLocalNetwork(applicationContext)) {
            Log.i(TAG, "background sync skipped: not on Wi-Fi or Ethernet")
            return@withContext Result.success()
        }
        val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
        // mDNS needs multicast, which Android filters without this lock.
        val lock = wifi.createMulticastLock("localpass-sync").apply {
            setReferenceCounted(false)
            acquire()
        }
        try {
            val summary = NativeBridge.runSyncOnce(
                applicationContext.filesDir.absolutePath,
                DeviceSecretStore.get(applicationContext),
                25_000,
            )
            Log.i(TAG, "background sync: $summary")
            Result.success()
        } catch (e: Throwable) {
            Log.w(TAG, "background sync failed", e)
            Result.retry()
        } finally {
            lock.release()
        }
    }

    companion object {
        private const val TAG = "LocalPass"
        private const val PERIODIC = "localpass-periodic-sync"
        private const val SOON = "localpass-sync-soon"

        /**
         * WorkManager's CONNECTED constraint also demands a working internet connection,
         * which a LAN-only network (guest VLAN, router without uplink) never has. So the
         * jobs carry no network constraint and check for Wi-Fi/Ethernet themselves.
         */
        @Suppress("DEPRECATION")
        private fun onLocalNetwork(context: Context): Boolean {
            val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE) as ConnectivityManager
            return cm.allNetworks.any { network ->
                val caps = cm.getNetworkCapabilities(network)
                caps != null && (caps.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) || caps.hasTransport(NetworkCapabilities.TRANSPORT_ETHERNET))
            }
        }

        fun schedulePeriodic(context: Context) {
            val request = PeriodicWorkRequestBuilder<SyncWorker>(15, TimeUnit.MINUTES).build()
            WorkManager.getInstance(context)
                .enqueueUniquePeriodicWork(PERIODIC, ExistingPeriodicWorkPolicy.KEEP, request)
        }

        /** One sync shortly after the app leaves the screen, to push recent edits. */
        fun scheduleSoon(context: Context) {
            val request = OneTimeWorkRequestBuilder<SyncWorker>()
                .setInitialDelay(5, TimeUnit.SECONDS)
                .build()
            WorkManager.getInstance(context).enqueueUniqueWork(SOON, ExistingWorkPolicy.REPLACE, request)
        }
    }
}
