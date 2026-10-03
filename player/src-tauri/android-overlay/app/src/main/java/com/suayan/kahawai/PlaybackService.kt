package com.suayan.kahawai

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
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
// media playback, which exempts it from that throttling.
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
  }

  private var wakeLock: PowerManager.WakeLock? = null

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
    // If the system kills us under memory pressure, restart while the user
    // still has something playing; the page re-syncs state on resume.
    return START_STICKY
  }

  override fun onDestroy() {
    wakeLock?.let { if (it.isHeld) it.release() }
    wakeLock = null
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
