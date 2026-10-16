package com.lanflare.desktop

import android.content.Context
import android.net.wifi.WifiManager
import android.os.Bundle
import android.webkit.JavascriptInterface
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import kotlin.math.roundToInt

class MainActivity : TauriActivity() {
  private var multicastLock: WifiManager.MulticastLock? = null

  private var safeTop: Int = 0
  private var safeBottom: Int = 0
  private var safeLeft: Int = 0
  private var safeRight: Int = 0

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)

    try {
      val wifi = applicationContext.getSystemService(Context.WIFI_SERVICE) as? WifiManager
      multicastLock = wifi?.createMulticastLock("lanflare_multicast_lock")
      multicastLock?.setReferenceCounted(true)
      multicastLock?.acquire()
    } catch (e: Exception) {
      e.printStackTrace()
    }
  }

  inner class InsetsBridge {
    @JavascriptInterface
    fun getTop(): Int = safeTop

    @JavascriptInterface
    fun getBottom(): Int = safeBottom

    @JavascriptInterface
    fun getLeft(): Int = safeLeft

    @JavascriptInterface
    fun getRight(): Int = safeRight
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)

    webView.addJavascriptInterface(InsetsBridge(), "AndroidSafeInsets")

    ViewCompat.setOnApplyWindowInsetsListener(webView) { _, insets ->
      val systemBars = insets.getInsets(WindowInsetsCompat.Type.systemBars())
      val displayCutout = insets.getInsets(WindowInsetsCompat.Type.displayCutout())

      val rawTop = maxOf(systemBars.top, displayCutout.top)
      val rawBottom = maxOf(systemBars.bottom, displayCutout.bottom)
      val rawLeft = maxOf(systemBars.left, displayCutout.left)
      val rawRight = maxOf(systemBars.right, displayCutout.right)

      val density = webView.resources.displayMetrics.density

      safeTop = if (density > 0) (rawTop / density).roundToInt() else 0
      safeBottom = if (density > 0) (rawBottom / density).roundToInt() else 0
      safeLeft = if (density > 0) (rawLeft / density).roundToInt() else 0
      safeRight = if (density > 0) (rawRight / density).roundToInt() else 0

      val script = """
        (function() {
          if (document.documentElement) {
            document.documentElement.style.setProperty('--safe-area-inset-top', '${safeTop}px');
            document.documentElement.style.setProperty('--safe-area-inset-bottom', '${safeBottom}px');
            document.documentElement.style.setProperty('--safe-area-inset-left', '${safeLeft}px');
            document.documentElement.style.setProperty('--safe-area-inset-right', '${safeRight}px');
          }
        })();
      """.trimIndent()

      webView.post {
        webView.evaluateJavascript(script, null)
      }

      insets
    }
  }

  override fun onDestroy() {
    super.onDestroy()
    try {
      if (multicastLock?.isHeld == true) {
        multicastLock?.release()
      }
    } catch (e: Exception) {
      e.printStackTrace()
    }
  }
}
