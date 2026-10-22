package it.termox.arvolo

import android.content.Intent
import android.os.Bundle
import androidx.activity.enableEdgeToEdge

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    // After `super`: that is what loads the Rust library `Shared` calls into.
    Shared.take(this, intent)
  }

  // `singleTask`: a share while the app is already up arrives here, not in a
  // second activity.
  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    Shared.take(this, intent)
  }
}
