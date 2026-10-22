package com.lanflare.desktop

import android.app.DownloadManager
import android.content.ActivityNotFoundException
import android.content.Context
import android.content.Intent
import android.media.MediaScannerConnection
import android.net.Uri
import android.net.wifi.WifiManager
import android.os.Build
import android.os.Bundle
import android.os.Environment
import android.provider.DocumentsContract
import android.provider.Settings
import android.webkit.JavascriptInterface
import android.webkit.MimeTypeMap
import android.webkit.WebView
import android.widget.Toast
import androidx.activity.enableEdgeToEdge
import androidx.core.content.FileProvider
import androidx.core.view.ViewCompat
import androidx.core.view.WindowInsetsCompat
import java.io.File
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

  inner class StorageBridge {
    @JavascriptInterface
    fun getPublicDownloadsDirectory(): String {
      @Suppress("DEPRECATION")
      return Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS).absolutePath
    }

    @JavascriptInterface
    fun isStorageManager(): Boolean {
      return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        Environment.isExternalStorageManager()
      } else {
        true
      }
    }

    @JavascriptInterface
    fun requestStoragePermission() {
      if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
        try {
          val intent = Intent(Settings.ACTION_MANAGE_APP_ALL_FILES_ACCESS_PERMISSION).apply {
            data = Uri.fromParts("package", packageName, null)
          }
          startActivity(intent)
        } catch (e: Exception) {
          try {
            val intent = Intent(Settings.ACTION_MANAGE_ALL_FILES_ACCESS_PERMISSION)
            startActivity(intent)
          } catch (e2: Exception) {
            e2.printStackTrace()
          }
        }
      }
    }

    @JavascriptInterface
    fun scanFile(path: String) {
      try {
        MediaScannerConnection.scanFile(applicationContext, arrayOf(path), null, null)
      } catch (e: Exception) {
        e.printStackTrace()
      }
    }

    @JavascriptInterface
    fun openFile(path: String) {
      val rawPath = path.trim()
      if (rawPath.isEmpty()) return
      val cleanPath = if (rawPath.startsWith("file://")) {
        Uri.parse(rawPath).path ?: rawPath.removePrefix("file://")
      } else {
        rawPath
      }

      val file = File(cleanPath)
      if (!file.exists()) {
        runOnUiThread {
          Toast.makeText(applicationContext, "File does not exist: ${file.name}", Toast.LENGTH_SHORT).show()
        }
        return
      }

      if (file.isDirectory) {
        openFolder(cleanPath)
        return
      }

      var mimeType: String? = null
      val ext = MimeTypeMap.getFileExtensionFromUrl(Uri.fromFile(file).toString())?.lowercase()
      if (!ext.isNullOrEmpty()) {
        mimeType = MimeTypeMap.getSingleton().getMimeTypeFromExtension(ext)
      }
      if (mimeType == null) {
        mimeType = java.net.URLConnection.guessContentTypeFromName(file.name)
      }
      if (mimeType == null) {
        mimeType = when (file.extension.lowercase()) {
          "apk" -> "application/vnd.android.package-archive"
          "pdf" -> "application/pdf"
          "epub" -> "application/epub+zip"
          "zip" -> "application/zip"
          "tar", "gz", "tgz" -> "application/gzip"
          "json" -> "application/json"
          "txt", "log", "md", "csv" -> "text/plain"
          "html", "htm" -> "text/html"
          "mp3", "wav", "flac", "ogg", "m4a", "aac" -> "audio/*"
          "mp4", "mkv", "avi", "mov", "webm", "3gp" -> "video/*"
          "jpg", "jpeg", "png", "gif", "webp", "bmp", "svg" -> "image/*"
          else -> "*/*"
        }
      }

      val contentUri: Uri = try {
        FileProvider.getUriForFile(
          applicationContext,
          "${packageName}.fileprovider",
          file
        )
      } catch (e: Exception) {
        e.printStackTrace()
        Uri.fromFile(file)
      }

      val intent = Intent(Intent.ACTION_VIEW).apply {
        setDataAndType(contentUri, mimeType)
        addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        addFlags(Intent.FLAG_GRANT_WRITE_URI_PERMISSION)
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      }

      runOnUiThread {
        try {
          startActivity(intent)
        } catch (e: ActivityNotFoundException) {
          if (mimeType != "*/*") {
            try {
              val fallbackIntent = Intent(Intent.ACTION_VIEW).apply {
                setDataAndType(contentUri, "*/*")
                addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
              }
              startActivity(Intent.createChooser(fallbackIntent, "Open with").apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
              })
            } catch (e2: Exception) {
              e2.printStackTrace()
              Toast.makeText(applicationContext, "No app available to open ${file.name}", Toast.LENGTH_SHORT).show()
            }
          } else {
            Toast.makeText(applicationContext, "No app available to open ${file.name}", Toast.LENGTH_SHORT).show()
          }
        } catch (e: Exception) {
          e.printStackTrace()
          Toast.makeText(applicationContext, "Failed to open file: ${e.message}", Toast.LENGTH_SHORT).show()
        }
      }
    }

    @JavascriptInterface
    fun openFolder(path: String) {
      val rawPath = path.trim()
      val cleanPath = if (rawPath.startsWith("file://")) {
        Uri.parse(rawPath).path ?: rawPath.removePrefix("file://")
      } else {
        rawPath
      }

      val file = File(cleanPath)
      val targetDir = if (file.exists() && file.isDirectory) {
        file
      } else if (file.parentFile != null && file.parentFile!!.exists()) {
        file.parentFile!!
      } else if (file.exists()) {
        file.parentFile ?: file
      } else {
        @Suppress("DEPRECATION")
        Environment.getExternalStoragePublicDirectory(Environment.DIRECTORY_DOWNLOADS)
      }

      runOnUiThread {
        var launched = false

        // Strategy 1: DocumentsUI external storage provider URI
        try {
          @Suppress("DEPRECATION")
          val extRoot = Environment.getExternalStorageDirectory().absolutePath
          val dirPath = targetDir.absolutePath

          val docId = if (dirPath.startsWith(extRoot)) {
            val relPath = dirPath.removePrefix(extRoot).trimStart('/')
            if (relPath.isEmpty()) "primary:" else "primary:$relPath"
          } else if (dirPath.startsWith("/storage/")) {
            val after = dirPath.removePrefix("/storage/")
            val parts = after.split("/", limit = 2)
            if (parts.size == 2) "${parts[0]}:${parts[1]}" else "${parts[0]}:"
          } else {
            "primary:Download"
          }

          val docUri = DocumentsContract.buildDocumentUri("com.android.externalstorage.documents", docId)
          val intent = Intent(Intent.ACTION_VIEW).apply {
            setDataAndType(docUri, DocumentsContract.Document.MIME_TYPE_DIR)
            addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
          }
          startActivity(intent)
          launched = true
        } catch (e: Exception) {
          // Fall through
        }

        // Strategy 2: If inside Download or Strategy 1 failed, launch DownloadManager.ACTION_VIEW_DOWNLOADS
        if (!launched && (targetDir.absolutePath.contains("/Download", ignoreCase = true) || targetDir.name.equals("Download", ignoreCase = true))) {
          try {
            val intent = Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).apply {
              addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
            }
            startActivity(intent)
            launched = true
          } catch (e: Exception) {
            // Fall through
          }
        }

        // Strategy 3: Try FileProvider URI with "resource/folder"
        if (!launched) {
          try {
            val contentUri = FileProvider.getUriForFile(
              applicationContext,
              "${packageName}.fileprovider",
              targetDir
            )
            val intent = Intent(Intent.ACTION_VIEW).apply {
              setDataAndType(contentUri, "resource/folder")
              addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
              addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            startActivity(Intent.createChooser(intent, "Open folder").apply {
              addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            })
            launched = true
          } catch (e: Exception) {
            // Fall through
          }
        }

        // Strategy 4: Fallback to DownloadManager.ACTION_VIEW_DOWNLOADS anyway
        if (!launched) {
          try {
            val intent = Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).apply {
              addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_CLEAR_TASK)
            }
            startActivity(intent)
            launched = true
          } catch (e: Exception) {
            // Fall through
          }
        }

        // Strategy 5: Storage Access Framework directory picker
        if (!launched) {
          try {
            val intent = Intent(Intent.ACTION_OPEN_DOCUMENT_TREE).apply {
              addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
            startActivity(intent)
          } catch (e: Exception) {
            Toast.makeText(applicationContext, "Cannot open file manager", Toast.LENGTH_SHORT).show()
          }
        }
      }
    }
  }

  override fun onWebViewCreate(webView: WebView) {
    super.onWebViewCreate(webView)

    webView.addJavascriptInterface(InsetsBridge(), "AndroidSafeInsets")
    webView.addJavascriptInterface(StorageBridge(), "AndroidStorage")


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
