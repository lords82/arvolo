//! Foto e video nella Galleria del telefono — come copia **in più**, mai al posto.
//!
//! Il design di riferimento (sezione 6) mette una regola prima di ogni schermata:
//! *il trasferimento non aspetta nessuna risposta*. Quello che arriva finisce
//! sempre e comunque nella cartella di Arvolo, pubblicato da
//! [`crate::publish_ios`] / [`crate::publish_android`] come qualsiasi altro file.
//! La Galleria è un secondo passo, deciso **dopo**, e che riguarda solo foto e
//! video: un PDF nella Galleria non lo vuole nessuno.
//!
//! Da qui la forma di questo modulo: non tocca la pubblicazione e non ha un
//! "modo". Sa fare due cose su un file che *esiste già* — mettercene una copia e
//! toglierla — e la preferenza che decide quando farlo vive nella GUI, perché è
//! lì che la domanda viene posta (schermata Finito) e lì che la risposta si
//! cambia (Impostazioni › Ricevuti).
//!
//! Le due piattaforme fanno la stessa cosa con mezzi diversi:
//!
//! - **iOS**: `PHPhotoLibrary` copia il file dentro la libreria Foto, che è un
//!   archivio a sé — l'originale in `Documents` resta dov'è. Serve il permesso
//!   *add-only* (`NSPhotoLibraryAddUsageDescription`), che è il più stretto che
//!   Apple offra: lascia aggiungere senza lasciar leggere la libreria.
//! - **Android**: una riga in `MediaStore.Images`/`Video` sotto `Pictures/Arvolo`
//!   o `Movies/Arvolo`, con lo stesso giro di `IS_PENDING` della pubblicazione in
//!   Download — e come quella, senza chiedere nessun permesso.
//!
//! Togliere è l'operazione asimmetrica: su iOS cancellare un asset fa comparire
//! la richiesta di conferma del sistema anche per gli asset creati dall'app, e
//! questo non si può evitare (né si vorrebbe).

// La finestra compila questo modulo per i tipi che i comandi restituiscono;
// le funzioni che leggono davvero esistono solo su un telefono.
#![cfg_attr(not(mobile), allow(dead_code))]

use std::path::Path;

/// Cosa la Galleria accetta. Il nome è l'unica cosa che abbiamo sempre: il file
/// è già stato pubblicato e su Android può non avere nemmeno un percorso.
///
/// Pura di proposito — è l'unica parte di questo modulo che un test su un
/// portatile può tenere a bada, ed è anche quella che sbaglia più facilmente.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Image,
    Video,
}

impl Kind {
    /// Il tipo MIME che Android vuole nella riga.
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn mime(self, ext: &str) -> String {
        match self {
            Kind::Image => match ext {
                "png" => "image/png".into(),
                "gif" => "image/gif".into(),
                "webp" => "image/webp".into(),
                "heic" | "heif" => "image/heic".into(),
                _ => "image/jpeg".into(),
            },
            Kind::Video => match ext {
                "mov" => "video/quicktime".into(),
                "webm" => "video/webm".into(),
                "m4v" => "video/x-m4v".into(),
                _ => "video/mp4".into(),
            },
        }
    }
}

/// Foto, video, o niente che la Galleria voglia.
///
/// L'elenco è corto apposta: sono i formati che le due gallerie indicizzano
/// davvero. Un `.raw` o un `.mkv` finiscono in un archivio che non li mostra, e
/// una copia invisibile è peggio di nessuna copia — il file resta in Arvolo, che
/// è dove la riga dell'app dice che sia.
pub fn kind_of(name: &str) -> Option<Kind> {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "heic" | "heif" => Some(Kind::Image),
        "mp4" | "mov" | "m4v" | "webm" => Some(Kind::Video),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_galleria_vuole_foto_e_video_e_nientaltro() {
        assert_eq!(kind_of("IMG_0001.HEIC"), Some(Kind::Image));
        assert_eq!(kind_of("foto.jpg"), Some(Kind::Image));
        assert_eq!(kind_of("clip.MOV"), Some(Kind::Video));
        // Il caso che la sezione 6 nomina per primo: i documenti non c'entrano.
        assert_eq!(kind_of("relazione.pdf"), None);
        assert_eq!(kind_of("archivio.zip"), None);
        assert_eq!(kind_of("senza-estensione"), None);
    }

    #[test]
    fn il_mime_segue_lestensione_e_ripiega_sul_piu_comune() {
        assert_eq!(Kind::Image.mime("png"), "image/png");
        assert_eq!(Kind::Image.mime("jpeg"), "image/jpeg");
        assert_eq!(Kind::Video.mime("mov"), "video/quicktime");
        assert_eq!(Kind::Video.mime("mp4"), "video/mp4");
    }
}

// ---------------------------------------------------------------- iOS -------

/// Copia `path` nella libreria Foto e restituisce l'identificatore locale
/// dell'asset creato — che è quello che serve per poterlo togliere.
#[cfg(target_os = "ios")]
pub fn add(path: &str) -> Result<String, String> {
    use objc2_foundation::{NSString, NSURL};
    use objc2_photos::{PHAssetChangeRequest, PHPhotoLibrary};

    let kind = kind_of(path).ok_or_else(|| "non è una foto né un video".to_string())?;
    allow_adding()?;

    let url = NSURL::fileURLWithPath(&NSString::from_str(path));
    // L'identificatore si prende *dentro* il blocco: il placeholder esiste solo
    // mentre la modifica è in corso, e dopo è un asset vero con lo stesso id.
    // Il blocco deve possedere quello che tocca — `performChangesAndWait:` lo
    // vuole senza prestiti — quindi l'id esce da una cella condivisa e non da
    // una variabile catturata.
    let id = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let out = std::sync::Arc::clone(&id);
    let changes = block2::RcBlock::new(move || {
        let req = match kind {
            Kind::Image => unsafe {
                PHAssetChangeRequest::creationRequestForAssetFromImageAtFileURL(&url)
            },
            Kind::Video => unsafe {
                PHAssetChangeRequest::creationRequestForAssetFromVideoAtFileURL(&url)
            },
        };
        if let Some(req) = req {
            if let Some(ph) = unsafe { req.placeholderForCreatedAsset() } {
                if let Ok(mut slot) = out.lock() {
                    *slot = unsafe { ph.localIdentifier() }.to_string();
                }
            }
        }
    });

    let library = unsafe { PHPhotoLibrary::sharedPhotoLibrary() };
    unsafe { library.performChangesAndWait_error(block_ptr(&changes)) }
        .map_err(|e| format!("la Galleria ha rifiutato il file: {e}"))?;

    let id = id
        .lock()
        .map_err(|_| "la Galleria non ha risposto".to_string())?
        .clone();
    if id.is_empty() {
        // Nessun placeholder: la libreria non ha creato niente e non lo ha
        // nemmeno detto. Senza id non c'è modo di annullare, quindi è un errore.
        return Err("la Galleria non ha creato niente".into());
    }
    Ok(id)
}

/// Toglie dalla Galleria la copia fatta da `add`. Il sistema chiede conferma:
/// cancellare un asset è una decisione dell'utente anche quando l'asset l'ha
/// creato questa app, e non c'è modo di saltare la domanda (né si vorrebbe).
#[cfg(target_os = "ios")]
pub fn remove(id: &str) -> Result<(), String> {
    use objc2_foundation::{NSArray, NSString};
    use objc2_photos::{PHAsset, PHAssetChangeRequest, PHPhotoLibrary};

    let ids = NSArray::from_retained_slice(&[NSString::from_str(id)]);
    let found = unsafe { PHAsset::fetchAssetsWithLocalIdentifiers_options(&ids, None) };
    if unsafe { found.count() } == 0 {
        // Già sparito: lo stato richiesto è quello che c'è.
        return Ok(());
    }
    let changes = block2::RcBlock::new(move || unsafe {
        // `deleteAssets` prende qualunque cosa si sappia enumerare, e un
        // risultato di fetch è esattamente quello.
        PHAssetChangeRequest::deleteAssets(objc2::runtime::ProtocolObject::from_ref(&*found));
    });
    let library = unsafe { PHPhotoLibrary::sharedPhotoLibrary() };
    unsafe { library.performChangesAndWait_error(block_ptr(&changes)) }
        .map_err(|e| format!("non sono riuscito a toglierla: {e}"))
}

/// Il puntatore che `performChangesAndWait:` si aspetta: un blocco di dispatch,
/// che in questa famiglia di crate è un `DynBlock<dyn Fn()>` grezzo. Il blocco
/// vive sullo stack del chiamante e la chiamata è sincrona, quindi il puntatore
/// non sopravvive a chi lo ha fatto.
#[cfg(target_os = "ios")]
fn block_ptr(b: &block2::DynBlock<dyn Fn()>) -> *mut block2::DynBlock<dyn Fn()> {
    (b as *const block2::DynBlock<dyn Fn()>).cast_mut()
}

/// Il permesso *add-only*: lascia aggiungere senza lasciar leggere la libreria.
/// Chiesto alla prima copia e mai più; una risposta negativa è definitiva fino a
/// che non la si cambia nelle impostazioni del sistema, e lo diciamo.
#[cfg(target_os = "ios")]
fn allow_adding() -> Result<(), String> {
    use objc2_photos::{PHAccessLevel, PHAuthorizationStatus, PHPhotoLibrary};

    let level = PHAccessLevel::AddOnly;
    let status = unsafe { PHPhotoLibrary::authorizationStatusForAccessLevel(level) };
    let status = if status == PHAuthorizationStatus::NotDetermined {
        // Bloccante di proposito: chi ha toccato "Galleria" sta guardando lo
        // schermo, e la copia deve avvenire adesso o non avvenire.
        let (tx, rx) = std::sync::mpsc::channel();
        let handler = block2::StackBlock::new(move |s: PHAuthorizationStatus| {
            let _ = tx.send(s);
        })
        .copy();
        unsafe { PHPhotoLibrary::requestAuthorizationForAccessLevel_handler(level, &handler) };
        rx.recv_timeout(std::time::Duration::from_secs(60))
            .unwrap_or(PHAuthorizationStatus::Denied)
    } else {
        status
    };

    match status {
        PHAuthorizationStatus::Authorized | PHAuthorizationStatus::Limited => Ok(()),
        _ => Err("Arvolo non ha il permesso di aggiungere alla Galleria".into()),
    }
}

// ------------------------------------------------------------ Android -------

/// La copia in Galleria su Android: una riga in `MediaStore.Images`/`Video`
/// sotto `Pictures/Arvolo` o `Movies/Arvolo`, con lo stesso giro di `IS_PENDING`
/// della pubblicazione in Download — e come quella, senza chiedere permessi.
///
/// L'origine può essere un percorso o un `content://`: un file già pubblicato
/// qui **non ha un percorso** (MediaStore restituisce una URI e basta), quindi
/// la copia si legge da dove c'è.
#[cfg(target_os = "android")]
pub fn add(src: &str) -> Result<String, String> {
    inner_add(src).map_err(|e| format!("{e:#}"))
}

#[cfg(target_os = "android")]
pub fn remove(id: &str) -> Result<(), String> {
    inner_remove(id).map_err(|e| format!("{e:#}"))
}

#[cfg(target_os = "android")]
fn inner_add(src: &str) -> anyhow::Result<String> {
    use anyhow::{anyhow, Context as _};
    use jni::objects::{JObject, JValue};

    let name = src.rsplit('/').next().unwrap_or(src).to_string();
    let kind = kind_of(&name).ok_or_else(|| anyhow!("non è una foto né un video"))?;
    let ext = std::path::Path::new(&name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let (vm, activity_ptr) = crate::publish_android::context()?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(activity_ptr.cast()) };
    let resolver = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;

    let (collection_class, relative) = match kind {
        Kind::Image => (
            "android/provider/MediaStore$Images$Media",
            "Pictures/Arvolo/",
        ),
        Kind::Video => ("android/provider/MediaStore$Video$Media", "Movies/Arvolo/"),
    };

    let values = env.new_object("android/content/ContentValues", "()V", &[])?;
    crate::publish_android::put_string(&mut env, &values, "_display_name", &name)?;
    crate::publish_android::put_string(&mut env, &values, "mime_type", &kind.mime(&ext))?;
    crate::publish_android::put_string(&mut env, &values, "relative_path", relative)?;
    crate::publish_android::put_int(&mut env, &values, "is_pending", 1)?;

    let collection = {
        let class = env.find_class(collection_class)?;
        env.get_static_field(class, "EXTERNAL_CONTENT_URI", "Landroid/net/Uri;")?
            .l()?
    };
    let uri = env
        .call_method(
            &resolver,
            "insert",
            "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
            &[JValue::Object(&collection), JValue::Object(&values)],
        )?
        .l()?;
    if uri.is_null() {
        return Err(anyhow!("la Galleria ha rifiutato una riga per {name}"));
    }

    let copied = copy_from(&mut env, &resolver, &uri, src);

    let done = env.new_object("android/content/ContentValues", "()V", &[])?;
    crate::publish_android::put_int(&mut env, &done, "is_pending", 0)?;
    match copied {
        Ok(()) => {
            env.call_method(
                &resolver,
                "update",
                "(Landroid/net/Uri;Landroid/content/ContentValues;Ljava/lang/String;[Ljava/lang/String;)I",
                &[
                    JValue::Object(&uri),
                    JValue::Object(&done),
                    JValue::Object(&JObject::null()),
                    JValue::Object(&JObject::null()),
                ],
            )?;
            let s = env
                .call_method(&uri, "toString", "()Ljava/lang/String;", &[])?
                .l()?;
            Ok(env.get_string(&s.into())?.into())
        }
        Err(e) => {
            // Mezza foto nella Galleria di qualcuno è peggio di nessuna foto.
            let _ = env.call_method(
                &resolver,
                "delete",
                "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I",
                &[
                    JValue::Object(&uri),
                    JValue::Object(&JObject::null()),
                    JValue::Object(&JObject::null()),
                ],
            );
            Err(e).context("copia in Galleria")
        }
    }
}

/// Versa `src` dentro la riga appena inserita. `src` può essere un percorso o
/// una `content://`: nel secondo caso i byte arrivano da uno stream del
/// resolver, perché è tutto quello che MediaStore concede su un file suo.
#[cfg(target_os = "android")]
fn copy_from(
    env: &mut jni::JNIEnv<'_>,
    resolver: &jni::objects::JObject<'_>,
    uri: &jni::objects::JObject<'_>,
    src: &str,
) -> anyhow::Result<()> {
    use anyhow::{anyhow, Context as _};
    use jni::objects::JValue;
    use std::io::Write as _;
    use std::os::fd::FromRawFd;

    let mode = env.new_string("w")?;
    let pfd = env
        .call_method(
            resolver,
            "openFileDescriptor",
            "(Landroid/net/Uri;Ljava/lang/String;)Landroid/os/ParcelFileDescriptor;",
            &[JValue::Object(uri), JValue::Object(&mode)],
        )?
        .l()?;
    if pfd.is_null() {
        return Err(anyhow!("nessun descrittore per la riga appena inserita"));
    }
    let fd = env.call_method(&pfd, "detachFd", "()I", &[])?.i()?;
    let mut out = unsafe { std::fs::File::from_raw_fd(fd) };

    if src.starts_with("content://") {
        // Da una URI: aprire lo stream, leggerlo a blocchi attraverso JNI. Più
        // lento di un descrittore, e inevitabile — MediaStore non presta un fd
        // in lettura per una riga di un'altra collezione.
        let parsed = {
            let s = env.new_string(src)?;
            let class = env.find_class("android/net/Uri")?;
            env.call_static_method(
                class,
                "parse",
                "(Ljava/lang/String;)Landroid/net/Uri;",
                &[JValue::Object(&s)],
            )?
            .l()?
        };
        let input = env
            .call_method(
                resolver,
                "openInputStream",
                "(Landroid/net/Uri;)Ljava/io/InputStream;",
                &[JValue::Object(&parsed)],
            )?
            .l()?;
        if input.is_null() {
            return Err(anyhow!("non riesco a leggere {src}"));
        }
        let buf = env.new_byte_array(64 * 1024)?;
        loop {
            let n = env
                .call_method(&input, "read", "([B)I", &[JValue::Object(&buf)])?
                .i()?;
            if n <= 0 {
                break;
            }
            let mut chunk = vec![0i8; n as usize];
            env.get_byte_array_region(&buf, 0, &mut chunk)?;
            // i8 e u8 sono lo stesso byte; la conversione è di tipo, non di dato.
            let bytes: &[u8] =
                unsafe { std::slice::from_raw_parts(chunk.as_ptr().cast(), chunk.len()) };
            out.write_all(bytes).context("scrittura in Galleria")?;
        }
        let _ = env.call_method(&input, "close", "()V", &[]);
    } else {
        let mut input = std::fs::File::open(src).with_context(|| format!("leggo {src}"))?;
        std::io::copy(&mut input, &mut out).context("copia in Galleria")?;
    }
    out.flush().context("flush in Galleria")?;
    Ok(())
}

#[cfg(target_os = "android")]
fn inner_remove(id: &str) -> anyhow::Result<()> {
    use jni::objects::{JObject, JValue};

    let (vm, activity_ptr) = crate::publish_android::context()?;
    let mut env = vm.attach_current_thread()?;
    let activity = unsafe { JObject::from_raw(activity_ptr.cast()) };
    let resolver = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;
    let parsed = {
        let s = env.new_string(id)?;
        let class = env.find_class("android/net/Uri")?;
        env.call_static_method(
            class,
            "parse",
            "(Ljava/lang/String;)Landroid/net/Uri;",
            &[JValue::Object(&s)],
        )?
        .l()?
    };
    env.call_method(
        &resolver,
        "delete",
        "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I",
        &[
            JValue::Object(&parsed),
            JValue::Object(&JObject::null()),
            JValue::Object(&JObject::null()),
        ],
    )?;
    Ok(())
}

// ---------------------------------------------- Le ultime foto (sezione 11) --

/// Una delle ultime foto del telefono, come la griglia di «Cosa» (11b) la
/// disegna: l'identificatore con cui [`export`] la trasforma in un file, e una
/// miniatura piccola — sono tessere, non immagini.
#[derive(serde::Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Recent {
    pub id: String,
    pub name: String,
    pub is_video: bool,
    /// Secondi, per un video; 0 per una foto.
    pub duration: f64,
    /// Un `data:` URL JPEG, 240px sul lato lungo.
    pub thumb: String,
}

/// Le ultime `limit` foto e video, dalla più recente. `None` dove non c'è una
/// libreria da leggere: la finestra, un telefono che ha detto di no.
#[cfg(target_os = "ios")]
pub fn recent(limit: usize, offset: usize) -> Result<Option<Vec<Recent>>, String> {
    use objc2::{msg_send, AllocAnyThread};
    use objc2_core_foundation::CGSize;
    use objc2_foundation::{NSArray, NSDictionary, NSSortDescriptor, NSString};
    use objc2_photos::{
        PHAsset, PHAssetMediaType, PHFetchOptions, PHImageContentMode, PHImageManager,
        PHImageRequestOptions, PHImageRequestOptionsDeliveryMode, PHImageRequestOptionsResizeMode,
    };
    use objc2_ui_kit::UIImage;

    if !allow_reading()? {
        return Ok(None);
    }

    let opts = unsafe { PHFetchOptions::init(PHFetchOptions::alloc()) };
    let by_date = NSSortDescriptor::sortDescriptorWithKey_ascending(
        Some(&NSString::from_str("creationDate")),
        false,
    );
    unsafe {
        opts.setSortDescriptors(Some(&NSArray::from_retained_slice(&[by_date])));
        // Il limite è *fino a dove arrivare*, non quante riportarne: PhotoKit
        // non ha uno scarto, ha una finestra che parte sempre dall'inizio. Si
        // chiede fino alla fine della pagina e si salta l'inizio sotto.
        opts.setFetchLimit(offset.saturating_add(limit));
    }
    let found = unsafe { PHAsset::fetchAssetsWithOptions(Some(&opts)) };
    let n = unsafe { found.count() };

    let manager = unsafe { PHImageManager::defaultManager() };
    let req = unsafe { PHImageRequestOptions::init(PHImageRequestOptions::alloc()) };
    unsafe {
        // Sincrono e veloce: sei tessere, chieste una dietro l'altra, e la
        // prima versione che c'è va benissimo per una tessera.
        req.setSynchronous(true);
        req.setDeliveryMode(PHImageRequestOptionsDeliveryMode::FastFormat);
        req.setResizeMode(PHImageRequestOptionsResizeMode::Fast);
        req.setNetworkAccessAllowed(false);
    }

    let mut out = Vec::with_capacity(n.saturating_sub(offset));
    for i in offset.min(n)..n {
        let asset = unsafe { found.objectAtIndex(i) };
        let media = unsafe { asset.mediaType() };
        if media != PHAssetMediaType::Image && media != PHAssetMediaType::Video {
            continue;
        }
        // La miniatura esce dal blocco per una cella condivisa, come l'id in
        // `add`: il blocco deve possedere quello che tocca.
        let slot = std::sync::Arc::new(std::sync::Mutex::new(None::<Vec<u8>>));
        let sink = std::sync::Arc::clone(&slot);
        let handler = block2::RcBlock::new(move |img: *mut UIImage, _info: *mut NSDictionary| {
            if img.is_null() {
                return;
            }
            let img = unsafe { &*img };
            if let Some(data) = img.jpeg_representation(0.62) {
                if let Ok(mut s) = sink.lock() {
                    *s = Some(data.to_vec());
                }
            }
        });
        // Il crate generato non espone la variante UIKit di questo metodo (solo
        // quella AppKit), quindi il selettore si manda a mano.
        let _id: i32 = unsafe {
            msg_send![
                &*manager,
                requestImageForAsset: &*asset,
                targetSize: CGSize { width: 240.0, height: 240.0 },
                contentMode: PHImageContentMode::AspectFill,
                options: Some(&*req),
                resultHandler: &*handler,
            ]
        };
        let Some(bytes) = slot.lock().ok().and_then(|s| s.clone()) else {
            continue;
        };
        out.push(Recent {
            id: unsafe { asset.localIdentifier() }.to_string(),
            name: resource_name(&asset).unwrap_or_else(|| format!("IMG_{}", i + 1)),
            is_video: media == PHAssetMediaType::Video,
            duration: if media == PHAssetMediaType::Video {
                unsafe { asset.duration() }
            } else {
                0.0
            },
            thumb: format!(
                "data:image/jpeg;base64,{}",
                data_encoding::BASE64.encode(&bytes)
            ),
        });
    }
    Ok(Some(out))
}

/// Il nome originale del file dietro un asset, se la libreria lo sa.
#[cfg(target_os = "ios")]
fn resource_name(asset: &objc2_photos::PHAsset) -> Option<String> {
    use objc2_photos::PHAssetResource;
    let resources = unsafe { PHAssetResource::assetResourcesForAsset(asset) };
    let first = resources.iter().next()?;
    Some(unsafe { first.originalFilename() }.to_string())
}

/// Scrive l'originale di un asset in un file temporaneo e restituisce il
/// percorso: è quello che l'engine sa mandare. Una copia, fatta solo al
/// momento di partire — la maggior parte delle tessere non parte mai.
#[cfg(target_os = "ios")]
pub fn export(id: &str) -> Result<std::path::PathBuf, String> {
    use objc2::AllocAnyThread;
    use objc2_foundation::{NSArray, NSError, NSString, NSURL};
    use objc2_photos::{
        PHAsset, PHAssetResource, PHAssetResourceManager, PHAssetResourceRequestOptions,
        PHAssetResourceType,
    };

    let ids = NSArray::from_retained_slice(&[NSString::from_str(id)]);
    let found = unsafe { PHAsset::fetchAssetsWithLocalIdentifiers_options(&ids, None) };
    let asset =
        unsafe { found.firstObject() }.ok_or_else(|| "quella foto non c'è più".to_string())?;
    let resources = unsafe { PHAssetResource::assetResourcesForAsset(&asset) };
    // L'originale: la foto o il video, non la versione modificata né le
    // tracce accessorie di una Live Photo.
    let resource = resources
        .iter()
        .find(|r| {
            let t = unsafe { r.r#type() };
            t == PHAssetResourceType::Photo || t == PHAssetResourceType::Video
        })
        .or_else(|| resources.iter().next())
        .ok_or_else(|| "quella foto non ha un file dietro".to_string())?;

    let name = unsafe { resource.originalFilename() }.to_string();
    let dir = std::env::temp_dir().join("arvolo-export");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let mut path = dir.join(&name);
    let mut k = 1;
    while path.exists() {
        let stem = std::path::Path::new(&name)
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| name.clone());
        let ext = std::path::Path::new(&name)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        path = dir.join(format!("{stem}-{k}{ext}"));
        k += 1;
    }

    let opts =
        unsafe { PHAssetResourceRequestOptions::init(PHAssetResourceRequestOptions::alloc()) };
    unsafe { opts.setNetworkAccessAllowed(true) };
    let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let done = block2::RcBlock::new(move |e: *mut NSError| {
        let r = if e.is_null() {
            Ok(())
        } else {
            Err(unsafe { &*e }.localizedDescription().to_string())
        };
        let _ = tx.send(r);
    });
    let manager = unsafe { PHAssetResourceManager::defaultManager() };
    unsafe {
        manager.writeDataForAssetResource_toFile_options_completionHandler(
            &resource,
            &url,
            Some(&opts),
            &done,
        )
    };
    // Bloccante: chi ha premuto «Manda» sta guardando lo schermo, e un video
    // su iCloud può metterci un po' — ma non all'infinito.
    rx.recv_timeout(std::time::Duration::from_secs(180))
        .map_err(|_| "la libreria non ha risposto in tempo".to_string())?
        .map_err(|e| format!("non sono riuscito a leggere quella foto: {e}"))?;
    Ok(path)
}

/// Il permesso di *leggere* la libreria, che `add` non chiede: serve solo
/// alla griglia delle ultime foto. `Ok(false)` è un no, e un no non è un
/// errore: la griglia semplicemente non c'è.
#[cfg(target_os = "ios")]
fn allow_reading() -> Result<bool, String> {
    use objc2_photos::{PHAccessLevel, PHAuthorizationStatus, PHPhotoLibrary};

    let level = PHAccessLevel::ReadWrite;
    let status = unsafe { PHPhotoLibrary::authorizationStatusForAccessLevel(level) };
    let status = if status == PHAuthorizationStatus::NotDetermined {
        let (tx, rx) = std::sync::mpsc::channel();
        let handler = block2::StackBlock::new(move |s: PHAuthorizationStatus| {
            let _ = tx.send(s);
        })
        .copy();
        unsafe { PHPhotoLibrary::requestAuthorizationForAccessLevel_handler(level, &handler) };
        rx.recv_timeout(std::time::Duration::from_secs(60))
            .unwrap_or(PHAuthorizationStatus::Denied)
    } else {
        status
    };
    Ok(matches!(
        status,
        PHAuthorizationStatus::Authorized | PHAuthorizationStatus::Limited
    ))
}

// ---------------------------------------------------------------- Android --
//
// La lettura vera sta in `mobile/android/Photos.kt`; qui c'è solo il filo che
// la chiama. Il confine è quello di `publish_android`: una query con un cursore,
// una miniatura per riga e un JSON in uscita si leggono meglio come codice
// Android che come una catena di `call_method`, e quello che resta da questa
// parte sono due chiamate statiche e un parse.

/// Chiama un metodo statico di `it.termox.arvolo.Photos` che prende il Context
/// e restituisce una stringa.
#[cfg(target_os = "android")]
fn call_photos(method: &str, sig: &str, extra: &[jni::objects::JValue]) -> Result<String, String> {
    use jni::objects::{JObject, JString, JValue};

    let (vm, activity_ptr) = crate::publish_android::context().map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let activity = unsafe { JObject::from_raw(activity_ptr.cast()) };

    let class = env
        .find_class("it/termox/arvolo/Photos")
        .map_err(|e| format!("Photos: {e}"))?;
    let mut args: Vec<JValue> = Vec::with_capacity(extra.len() + 1);
    args.push(JValue::Object(&activity));
    args.extend(extra.iter().cloned());
    let out = env
        .call_static_method(class, method, sig, &args)
        .map_err(|e| format!("{method}: {e}"))?;
    let s: JString = out.l().map_err(|e| e.to_string())?.into();
    env.get_string(&s)
        .map(|v| v.into())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "android")]
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawRecent {
    id: String,
    name: String,
    is_video: bool,
    duration: f64,
    thumb: String,
}

#[cfg(target_os = "android")]
pub fn recent(limit: usize, offset: usize) -> Result<Option<Vec<Recent>>, String> {
    use jni::objects::JValue;
    let json = call_photos(
        "recent",
        "(Landroid/content/Context;II)Ljava/lang/String;",
        &[JValue::Int(limit as i32), JValue::Int(offset as i32)],
    )?;
    let raw: Vec<RawRecent> = serde_json::from_str(&json).map_err(|e| e.to_string())?;
    // Vuoto non è «nessuna libreria»: è una libreria senza altro da dare, che a
    // pagina due è la cosa normale. `None` lo dice solo chi non può leggere.
    Ok(Some(
        raw.into_iter()
            .map(|r| Recent {
                id: r.id,
                name: r.name,
                is_video: r.is_video,
                duration: r.duration,
                thumb: r.thumb,
            })
            .collect(),
    ))
}

#[cfg(target_os = "android")]
pub fn export(id: &str) -> Result<std::path::PathBuf, String> {
    use jni::objects::{JObject, JString, JValue};

    // Tutto dentro un solo attacco al VM: la stringa che si crea qui è una
    // reference locale, viva finché vive il frame che l'ha creata. Crearla con
    // un `env` e usarla con un altro è il modo di avere un riferimento che
    // punta a niente — e su JNI non è un errore, è un crash più tardi.
    let (vm, activity_ptr) = crate::publish_android::context().map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let activity = unsafe { JObject::from_raw(activity_ptr.cast()) };
    let jid = env.new_string(id).map_err(|e| e.to_string())?;
    let class = env
        .find_class("it/termox/arvolo/Photos")
        .map_err(|e| format!("Photos: {e}"))?;
    let out = env
        .call_static_method(
            class,
            "export",
            "(Landroid/content/Context;Ljava/lang/String;)Ljava/lang/String;",
            &[JValue::Object(&activity), JValue::Object(&jid)],
        )
        .map_err(|e| format!("export: {e}"))?;
    let s: JString = out.l().map_err(|e| e.to_string())?.into();
    let path: String = env.get_string(&s).map_err(|e| e.to_string())?.into();
    Ok(std::path::PathBuf::from(path))
}

/// Il foglio di condivisione del sistema con dentro un indirizzo (18c).
///
/// Sta in questo file e non in `ios_open` perché qui c'è già il filo verso
/// Kotlin, ed è lo stesso: una chiamata statica con il Context davanti.
#[cfg(target_os = "android")]
pub fn share_text(text: &str) -> Result<(), String> {
    use jni::objects::{JObject, JValue};

    let (vm, activity_ptr) = crate::publish_android::context().map_err(|e| e.to_string())?;
    let mut env = vm.attach_current_thread().map_err(|e| e.to_string())?;
    let activity = unsafe { JObject::from_raw(activity_ptr.cast()) };
    let jtext = env.new_string(text).map_err(|e| e.to_string())?;
    let class = env
        .find_class("it/termox/arvolo/Photos")
        .map_err(|e| format!("Photos: {e}"))?;
    env.call_static_method(
        class,
        "shareText",
        "(Landroid/content/Context;Ljava/lang/String;)V",
        &[JValue::Object(&activity), JValue::Object(&jtext)],
    )
    .map_err(|e| format!("shareText: {e}"))?;
    Ok(())
}
