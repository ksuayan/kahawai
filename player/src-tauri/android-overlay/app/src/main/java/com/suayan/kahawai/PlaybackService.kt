package com.suayan.kahawai

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat

// Kept in player/src-tauri/android-overlay and copied over the generated
// project (player/src-tauri/gen/android, gitignored) by scripts/build-android.sh
// and scripts/start-dev-android.sh. Edit this copy, not the generated one.
//
// What this is for: without a foreground service, Android treats the player
// as a background app once the screen goes off or the user switches away,
// and Doze / app-standby throttling starves the Rust audio thread. The
// result is the breakup and stuttering heard on the HiBy R4 after some
// minutes of playback. Running as a mediaPlayback foreground service (plus a
// partial wake lock while playing) tells the OS this process is doing active
// media playback, which exempts it from that throttling. A Wi-Fi lock keeps
// the radio in low-latency mode with the screen off: the wake lock protects
// the CPU, but without the Wi-Fi lock the radio itself can doze and starve
// the stream (the player is a pure streaming client: server, radio and
// podcasts all arrive over the network).
//
// Driven from the page through window.KahawaiPlayback.setActive(active,
// title, artist) (player/ui/src/lib/playbackService.ts): the player store
// calls it whenever playback starts or stops. The audio itself keeps running
// in the Rust core in this process; the service only keeps the process alive.
class PlaybackService : Service() {
  companion object {
    const val EXTRA_ACTIVE = "active"
    const val EXTRA_TITLE = "title"
    const val EXTRA_ARTIST = "artist"
    private const val NOTIFICATION_ID = 1
    private const val CHANNEL_ID = "kahawai_playback"
    private const val WAKE_LOCK_TAG = "Kahawai:playback"
    private const val WIFI_LOCK_TAG = "Kahawai:streaming"
  }

  private var wakeLock: PowerManager.WakeLock? = null
  private var wifiLock: WifiManager.WifiLock? = null

  override fun onCreate() {
    super.onCreate()
    // Notification channels are required on API 26+ (minSdk is 26).
    val manager = getSystemService(Context.NOTIFICATION_SERVICE) as NotificationManager
    manager.createNotificationChannel(
      NotificationChannel(CHANNEL_ID, "Playback", NotificationManager.IMPORTANCE_LOW)
    )
  }

  override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
    val active = intent?.getBooleanExtra(EXTRA_ACTIVE, true) ?: true
    if (!active) {
      stopSelf()
      return START_NOT_STICKY
    }
    val title = intent?.getStringExtra(EXTRA_TITLE)?.takeIf { it.isNotBlank() }
      ?: getString(R.string.app_name)
    val artist = intent?.getStringExtra(EXTRA_ARTIST)?.takeIf { it.isNotBlank() }

    // The manifest declares android:foregroundServiceType="mediaPlayback",
    // so the two-arg startForeground is correct on every API level.
    startForeground(NOTIFICATION_ID, buildNotification(title, artist))

    // Keep the CPU running with the screen off. Reference-counted off: one
    // acquire here, one release in onDestroy, no leak across play cycles.
    if (wakeLock == null) {
      val power = getSystemService(Context.POWER_SERVICE) as PowerManager
      wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, WAKE_LOCK_TAG).also {
        it.setReferenceCounted(false)
        it.acquire()
      }
    }
    // Keep the Wi-Fi radio in low-latency mode with the screen off. The wake
    // lock protects the CPU; without this, the radio itself can doze and the
    // stream starves even though the audio thread is running. No extra
    // manifest permission: WifiLock is covered by WAKE_LOCK. Like the wake
    // lock: one acquire here, one release in onDestroy.
    if (wifiLock == null) {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as WifiManager
      // LOW_LATENCY is the API 29+ mode (less power than HIGH_PERF, same
      // goal); HIGH_PERF auto-maps to it on API 34+, but spell it out.
      val mode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) {
        WifiManager.WIFI_MODE_FULL_LOW_LATENCY
      } else {
        @Suppress("DEPRECATION")
        WifiManager.WIFI_MODE_FULL_HIGH_PERF
      }
      wifiLock = wifi.createWifiLock(mode, WIFI_LOCK_TAG).also {
        it.setReferenceCounted(false)
        it.acquire()
      }
    }
    // If the system kills us under memory pressure, restart while the user
    // still has something playing; the page re-syncs state on resume.
    return START_STICKY
  }

  override fun onDestroy() {
    wakeLock?.let { if (it.isHeld) it.release() }
    wakeLock = null
    wifiLock?.let { if (it.isHeld) it.release() }
    wifiLock = null
    super.onDestroy()
  }

  override fun onBind(intent: Intent?): IBinder? = null

  private fun buildNotification(title: String, artist: String?): Notification {
    val launch = packageManager.getLaunchIntentForPackage(packageName)?.let {
      PendingIntent.getActivity(
        this, 0, it,
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
      )
    }
    return NotificationCompat.Builder(this, CHANNEL_ID)
      .setContentTitle(title)
      .setContentText(artist ?: getString(R.string.app_name))
      .setSmallIcon(R.mipmap.ic_launcher)
      .setContentIntent(launch)
      .setOngoing(true)
      // A media-style notification; transport controls ride on the media
      // session follow-up, not on this first pass.
      .setCategory(NotificationCompat.CATEGORY_TRANSPORT)
      .build()
  }
}
