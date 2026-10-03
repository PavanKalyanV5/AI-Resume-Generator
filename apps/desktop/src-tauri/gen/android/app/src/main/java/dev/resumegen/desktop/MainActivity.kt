package dev.resumegen.desktop

import android.content.Intent
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
import org.json.JSONObject
import java.io.File
import java.net.HttpURLConnection
import java.net.URL

class MainActivity : TauriActivity() {
  private var web: WebView? = null
  private var pending: String? = null

  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    pending = sharedText(intent)
  }

  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    sharedText(intent)?.let { deliver(it) }
  }

  override fun onWebViewCreate(webView: WebView) {
    web = webView
    pending?.let { deliver(it) }
    pending = null
  }

  private fun sharedText(i: Intent?): String? =
    if (i?.action == Intent.ACTION_SEND && i.type == "text/plain") i.getStringExtra(Intent.EXTRA_TEXT) else null

  // The Rust side writes "port\nsecret" (private, 0600) to companion.port in the app data dir (exact dir depends on Tauri).
  private fun proxyPortKey(): Pair<Int, String>? =
    listOf(filesDir, dataDir, File(filesDir, packageName), File(dataDir, packageName))
      .map { File(it, "companion.port") }.firstOrNull { it.exists() }?.readText()?.trim()?.lines()
      ?.let { l -> l.getOrNull(0)?.toIntOrNull()?.let { p -> l.getOrNull(1)?.let { p to it } } }

  /** Hand the shared text to the local proxy, then reload the UI at Intake (it picks the text up from /companion/share). */
  private fun deliver(text: String) {
    Thread {
      val (port, key) = proxyPortKey() ?: return@Thread
      try {
        (URL("http://127.0.0.1:$port/companion/share").openConnection() as HttpURLConnection).run {
          requestMethod = "POST"; doOutput = true; setRequestProperty("Content-Type", "application/json"); setRequestProperty("x-companion-key", key)
          outputStream.use { it.write(JSONObject().put("text", text).toString().toByteArray()) }
          responseCode; disconnect()
        }
      } catch (_: Exception) { return@Thread }
      runOnUiThread { web?.loadUrl("http://127.0.0.1:$port/") }
    }.start()
  }
}
