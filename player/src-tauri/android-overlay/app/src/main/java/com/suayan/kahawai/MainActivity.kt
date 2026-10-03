package com.suayan.kahawai

import android.content.Intent
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.OnBackPressedCallback
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat

// Kept in player/src-tauri/android-overlay and copied over the generated
// project (player/src-tauri/gen/android, gitignored) by scripts/build-android.sh
// and start-dev-android.sh. Edit this copy, not the generated one.
//
// The app draws edge to edge (under the status and navigation bars). Older
// Android WebViews (the HiBy R4's Chromium 91) report every
// env(safe-area-inset-*) as 0, so the page cannot tell how much room the bars
// take: the heading slides under the status bar and the tab bar under the
// gesture handle. This tells the page the real insets, in CSS pixels, through
// window.KahawaiInsets (player/ui/src/lib/insets.ts), and nudges it whenever
// they change (rotation, the keyboard).
class MainActivity : TauriActivity() {
  @Volatile private var insetsJson = "{\"top\":0,\"right\":0,\"bottom\":0,\"left\":0}"

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)
    webView.addJavascriptInterface(InsetsBridge(), "KahawaiInsets")
    webView.addJavascriptInterface(PlaybackBridge(), "KahawaiPlayback")
    handleBack(webView)
    ViewCompat.setOnApplyWindowInsetsListener(webView) { view, insets ->
      val bars = insets.getInsets(WindowInsetsCompat.Type.systemBars() or WindowInsetsCompat.Type.displayCutout())
      val density = view.resources.displayMetrics.density
      fun css(px: Int) = Math.round(px / density)
      insetsJson = "{\"top\":${css(bars.top)},\"right\":${css(bars.right)},\"bottom\":${css(bars.bottom)},\"left\":${css(bars.left)}}"
      view.post {
        (view as WebView).evaluateJavascript("window.dispatchEvent(new Event('kahawai-insets'))", null)
      }
      insets
    }
  }

  // Back asks the page first (player/ui/src/lib/backButton.ts): it closes
  // the open sheet, dialog or menu, or goes back a step of the breadcrumb.
  // The page keeps no browser history, so the generated WryActivity's own
  // handler (WebView history, else close) shut the app on the first press.
  // When the page has nothing to undo, the app goes to the background, like
  // Home: playback carries on.
  //
  // Every WebView passes through here (the splash's too, which stays alive
  // hidden), and WryActivity adds its own callback for each one. The last
  // callback added runs first, so ours is moved to the end every time; it
  // asks each WebView in turn until one has the hook.
  private val webViews = mutableListOf<WebView>()
  private val backCallback = object : OnBackPressedCallback(true) {
    override fun handleOnBackPressed() = askPage(0)
  }

  private fun handleBack(webView: WebView) {
    webViews.add(webView)
    backCallback.remove()
    onBackPressedDispatcher.addCallback(this, backCallback)
  }

  private fun askPage(index: Int) {
    val webView = webViews.getOrNull(index)
    if (webView == null) {
      moveTaskToBack(true)
      return
    }
    try {
      webView.evaluateJavascript("window.kahawaiBack ? String(window.kahawaiBack() === true) : 'none'") { answer ->
        when (answer) {
          "\"true\"" -> {}
          "\"false\"" -> moveTaskToBack(true)
          else -> askPage(index + 1)
        }
      }
    } catch (_: Exception) {
      askPage(index + 1) // a WebView already torn down
    }
  }

  private inner class InsetsBridge {
    @JavascriptInterface
    fun get(): String = insetsJson
  }

  // Playback state from the page (player/ui/src/lib/playbackService.ts):
  // starting the foreground service keeps the Rust audio thread exempt from
  // Doze / app-standby throttling while something is playing. Calls arrive on
  // the WebView's JavaBridge thread; start/stopService are safe from there.
  private inner class PlaybackBridge {
    @JavascriptInterface
    fun setActive(active: Boolean, title: String?, artist: String?) {
      val intent = Intent(this@MainActivity, PlaybackService::class.java).apply {
        putExtra(PlaybackService.EXTRA_ACTIVE, active)
        putExtra(PlaybackService.EXTRA_TITLE, title)
        putExtra(PlaybackService.EXTRA_ARTIST, artist)
      }
      if (active) {
        try {
          // Must be called while the app is in the foreground; that is the
          // only path that reaches "playing" (the user pressed play).
          ContextCompat.startForegroundService(this@MainActivity, intent)
        } catch (_: Exception) {
          // API 31+ throws when starting a foreground service from the
          // background. If the service is already running (track changed
          // while backgrounded) a plain start still delivers the intent so
          // the notification text updates.
          try {
            startService(intent)
          } catch (_: Exception) {
          }
        }
      } else {
        try {
          stopService(intent)
        } catch (_: Exception) {
        }
      }
    }
  }
}
