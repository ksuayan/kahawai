package com.suayan.kahawai

import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
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

  private inner class InsetsBridge {
    @JavascriptInterface
    fun get(): String = insetsJson
  }
}
