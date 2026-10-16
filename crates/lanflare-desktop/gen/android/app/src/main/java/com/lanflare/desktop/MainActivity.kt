package com.lanflare.desktop

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
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
    handleShareIntent(intent)
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

  override fun onNewIntent(intent: Intent) {
    handleShareIntent(intent)
    super.onNewIntent(intent)
  }

  private fun handleShareIntent(intent: Intent?) {
    if (intent == null) return

    val action = intent.action
    val type = intent.type ?: "*/*"

    if (action == Intent.ACTION_SEND) {
      // If EXTRA_STREAM is a Parcelable Uri, convert to String so intent.toUri(0) won't drop it
      val streamUri = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        intent.getParcelableExtra(Intent.EXTRA_STREAM, Uri::class.java)
      } else {
        @Suppress("DEPRECATION")
        intent.getParcelableExtra(Intent.EXTRA_STREAM) as? Uri
      }

      if (streamUri != null) {
        intent.putExtra(Intent.EXTRA_STREAM, streamUri.toString())
      } else if (intent.clipData != null && intent.clipData!!.itemCount > 0) {
        val clipUri = intent.clipData!!.getItemAt(0).uri
        if (clipUri != null) {
          intent.putExtra(Intent.EXTRA_STREAM, clipUri.toString())
        }
      }
    } else if (action == Intent.ACTION_SEND_MULTIPLE) {
      val streamList = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM, Uri::class.java)
      } else {
        @Suppress("DEPRECATION")
        intent.getParcelableArrayListExtra(Intent.EXTRA_STREAM)
      }

      val uris = mutableListOf<Uri>()
      if (streamList != null) {
        uris.addAll(streamList)
      }
      if (uris.isEmpty() && intent.clipData != null) {
        for (i in 0 until intent.clipData!!.itemCount) {
          val u = intent.clipData!!.getItemAt(i).uri
          if (u != null) {
            uris.add(u)
          }
        }
      }

      if (uris.isNotEmpty()) {
        try {
          val shareTarget = com.plugin.mobilesharetarget.Sharetarget()
          for (u in uris) {
            val encodedUri = Uri.encode(u.toString())
            val rawIntent = "#Intent;action=android.intent.action.SEND;type=${type};S.android.intent.extra.STREAM=${encodedUri};end"
            shareTarget.pushIntent(rawIntent)
          }
        } catch (e: Exception) {
          e.printStackTrace()
        }
      }
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
