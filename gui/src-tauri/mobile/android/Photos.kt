package it.termox.arvolo

import android.content.ContentUris
import android.content.Context
import android.graphics.Bitmap
import android.os.Build
import android.provider.MediaStore
import android.util.Base64
import android.util.Size
import org.json.JSONArray
import org.json.JSONObject
import java.io.ByteArrayOutputStream
import java.io.File

/**
 * La libreria del telefono, su Android.
 *
 * Il gemello di `photos.rs` per iOS, e in Kotlin invece che in JNI per una
 * ragione scritta in `publish_android.rs`: là MediaStore sono quattro chiamate e
 * si fanno da Rust; qui sono una query con un cursore, una miniatura per riga,
 * un JPEG per miniatura e un JSON in uscita — abbastanza perché valga la pena
 * leggerlo come codice Android invece che come una catena di `call_method`.
 *
 * Due cose che su Android non sono come su iOS e si vedono qui dentro:
 *
 * - **Nessun permesso da chiedere, finché si guarda e basta.** `MediaStore` si
 *   interroga senza `READ_MEDIA_IMAGES` quando a scegliere è l'utente; quello
 *   che serve è leggere *il file scelto*, e lì il permesso lo dà il content
 *   resolver riga per riga.
 * - **La miniatura è del sistema.** `loadThumbnail` (API 29+) restituisce quella
 *   che il sistema ha già in cache: niente decodifica dell'originale, che su una
 *   foto da 12 megapixel sarebbe un decimo di secondo per tessera.
 */
object Photos {
    /** Sotto questa API non c'è `loadThumbnail` e la griglia resta vuota. */
    private const val MIN_API = Build.VERSION_CODES.Q

    /**
     * Una pagina di foto e video, dalla più recente, come JSON:
     * `[{id, name, isVideo, duration, thumb}]` con `thumb` già `data:` URL.
     *
     * `limit` e `offset` sono la finestra. Android non ha un `LIMIT` portatile
     * nel `sortOrder` (su API 30+ il content resolver lo rifiuta), quindi si
     * usa il `Bundle` di argomenti quando c'è e si taglia a mano quando non c'è.
     */
    @JvmStatic
    fun recent(ctx: Context, limit: Int, offset: Int): String {
        val out = JSONArray()
        if (Build.VERSION.SDK_INT < MIN_API) return out.toString()

        val uri = MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL)
        val cols = arrayOf(
            MediaStore.Files.FileColumns._ID,
            MediaStore.Files.FileColumns.DISPLAY_NAME,
            MediaStore.Files.FileColumns.MEDIA_TYPE,
            MediaStore.Files.FileColumns.DURATION,
        )
        // Solo immagini e video: MediaStore.Files tiene dentro anche i documenti
        // e gli audio, che qui non sono «la galleria».
        val sel = "${MediaStore.Files.FileColumns.MEDIA_TYPE} = ? OR " +
            "${MediaStore.Files.FileColumns.MEDIA_TYPE} = ?"
        val args = arrayOf(
            MediaStore.Files.FileColumns.MEDIA_TYPE_IMAGE.toString(),
            MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO.toString(),
        )
        val order = "${MediaStore.Files.FileColumns.DATE_ADDED} DESC"

        val cursor = ctx.contentResolver.query(uri, cols, sel, args, order) ?: return out.toString()
        cursor.use { c ->
            val idCol = c.getColumnIndexOrThrow(MediaStore.Files.FileColumns._ID)
            val nameCol = c.getColumnIndexOrThrow(MediaStore.Files.FileColumns.DISPLAY_NAME)
            val typeCol = c.getColumnIndexOrThrow(MediaStore.Files.FileColumns.MEDIA_TYPE)
            val durCol = c.getColumnIndexOrThrow(MediaStore.Files.FileColumns.DURATION)
            if (!c.moveToPosition(offset)) return out.toString()
            var taken = 0
            do {
                if (taken >= limit) break
                val id = c.getLong(idCol)
                val item = ContentUris.withAppendedId(uri, id)
                val thumb = try {
                    val bmp = ctx.contentResolver.loadThumbnail(item, Size(240, 240), null)
                    val buf = ByteArrayOutputStream()
                    bmp.compress(Bitmap.CompressFormat.JPEG, 62, buf)
                    bmp.recycle()
                    "data:image/jpeg;base64," + Base64.encodeToString(buf.toByteArray(), Base64.NO_WRAP)
                } catch (e: Exception) {
                    // Una riga senza miniatura non è una pagina senza foto: il
                    // file può essere su una scheda smontata, o essere arrivato
                    // mezzo. Si salta quella e si va avanti.
                    android.util.Log.w("arvolo", "no thumbnail for $id: $e")
                    continue
                }
                val isVideo = c.getInt(typeCol) == MediaStore.Files.FileColumns.MEDIA_TYPE_VIDEO
                out.put(
                    JSONObject()
                        .put("id", id.toString())
                        .put("name", c.getString(nameCol) ?: "")
                        .put("isVideo", isVideo)
                        // Il millisecondo è di MediaStore, il secondo è di chi legge.
                        .put("duration", if (isVideo) c.getLong(durCol) / 1000.0 else 0.0)
                        .put("thumb", thumb)
                )
                taken++
            } while (c.moveToNext())
        }
        return out.toString()
    }

    /**
     * La foto vera, copiata nella cartella dell'app, e il percorso di dove è
     * finita. Copiata e non riferita: un `content://` vale finché vale il
     * permesso che l'ha aperto, e un invio comincia dopo e dura.
     */
    @JvmStatic
    fun export(ctx: Context, id: String): String {
        val uri = ContentUris.withAppendedId(
            MediaStore.Files.getContentUri(MediaStore.VOLUME_EXTERNAL),
            id.toLong()
        )
        var name = "foto"
        ctx.contentResolver.query(
            uri,
            arrayOf(MediaStore.Files.FileColumns.DISPLAY_NAME),
            null, null, null
        )?.use { c ->
            if (c.moveToFirst()) c.getString(0)?.takeIf { it.isNotBlank() }?.let { name = it }
        }
        val dir = File(ctx.filesDir, "picked")
        dir.mkdirs()
        var out = File(dir, name)
        var n = 2
        val stem = name.substringBeforeLast('.', name)
        val ext = if (name.contains('.')) "." + name.substringAfterLast('.') else ""
        while (out.exists()) { out = File(dir, "$stem-$n$ext"); n++ }
        ctx.contentResolver.openInputStream(uri)?.use { input ->
            out.outputStream().use { input.copyTo(it) }
        } ?: throw IllegalStateException("non si apre $uri")
        return out.absolutePath
    }

    /**
     * Il foglio di condivisione del sistema, con dentro un indirizzo (18c).
     *
     * `FLAG_ACTIVITY_NEW_TASK` perché questo parte da una chiamata nativa e non
     * da un gesto dentro un'activity: senza, su alcune versioni il sistema
     * rifiuta di aprirlo.
     */
    @JvmStatic
    fun shareText(ctx: Context, text: String) {
        val send = android.content.Intent(android.content.Intent.ACTION_SEND).apply {
            type = "text/plain"
            putExtra(android.content.Intent.EXTRA_TEXT, text)
        }
        val chooser = android.content.Intent.createChooser(send, null)
        chooser.addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
        ctx.startActivity(chooser)
    }
}
