//! Putting a finished download where a person can find it, on Android.
//!
//! A download lands in the app's own storage because that is the only place a
//! receive can be written the way this engine writes one — pre-allocated, filled
//! at offsets, with a resume sidecar beside it (see
//! [`arvolo_core::manager::SavePublisher`] for why none of that survives inside
//! MediaStore). What is left is to move it, once, when it is whole.
//!
//! The destination is `Download/Arvolo/`, through MediaStore. That is the one
//! public place an app can write on API 29+ **without asking for a permission**:
//! no `WRITE_EXTERNAL_STORAGE`, no `MANAGE_EXTERNAL_STORAGE` — the second of
//! which would be a conversation with the Play Store review team, for a file
//! manager's privilege this app has no business holding.
//!
//! No Kotlin. The platform side of this is four calls — insert a pending row,
//! open its descriptor, copy, clear the pending flag — and Rust can make them
//! through JNI directly, using the VM and activity `tao` already holds. A plugin
//! would be a Gradle project, a Kotlin file and a wire format for the same four
//! calls; the reason to write one is when the platform work is large enough to
//! be worth reading in Kotlin, and this is not.
//!
//! `IS_PENDING` is the important detail: the row exists but is invisible to other
//! apps until the copy has finished, so nothing can open a half-written file, and
//! a crash mid-copy leaves a pending row the system reaps rather than a truncated
//! download in somebody's Downloads.

#[cfg(target_os = "android")]
use std::path::Path;
use std::path::PathBuf;

#[cfg(target_os = "android")]
use anyhow::{anyhow, Context, Result};
#[cfg(target_os = "android")]
use arvolo_core::manager::{SavePublisher, SavedLocation};
#[cfg(target_os = "android")]
use jni::objects::{JObject, JValue};
#[cfg(target_os = "android")]
use jni::JavaVM;

#[cfg(target_os = "android")]
/// The folder inside the user's Downloads. One level, named after the app: a
/// person looking for what arrived should find it all together, and a person
/// uninstalling the app should be able to keep it.
const RELATIVE_PATH: &str = "Download/Arvolo/";

#[cfg(target_os = "android")]
/// MediaStore's public Downloads collection landed in API 29. Below that an app
/// cannot write anywhere public without a permission this one refuses to ask
/// for, so there the file stays in app storage and the row says so.
const API_Q: i32 = 29;

#[cfg(target_os = "android")]
pub struct MediaStorePublisher;

#[cfg(target_os = "android")]
impl SavePublisher for MediaStorePublisher {
    fn publish(&self, staged: &Path, name: &str) -> Result<SavedLocation> {
        if staged.is_dir() {
            // An archive arrives unpacked, as a folder. Publishing a tree into
            // MediaStore means one row per file and a name for the folder that
            // no longer exists — a different job, and one nobody has asked for
            // yet. It stays where it is, whole, and the row still points at it.
            return Err(anyhow!(
                "a folder cannot be published to Downloads; it stays in the app"
            ));
        }
        publish_file(staged, name)
    }
}

#[cfg(target_os = "android")]
/// The VM and the activity, from what the windowing layer already holds. Both
/// are process-wide and outlive any transfer.
pub(crate) fn context() -> Result<(JavaVM, *mut std::ffi::c_void)> {
    use tao::platform::android::prelude::*;
    let ctx = main_android_context().context("no Android context yet")?;
    let vm = unsafe { JavaVM::from_raw(ctx.java_vm.cast()) }.context("attach to the VM")?;
    Ok((vm, ctx.context_jobject))
}

#[cfg(target_os = "android")]
fn publish_file(staged: &Path, name: &str) -> Result<SavedLocation> {
    let (vm, activity_ptr) = context()?;
    let mut env = vm
        .attach_current_thread()
        .context("attach this thread to the VM")?;
    let activity = unsafe { JObject::from_raw(activity_ptr.cast()) };

    // API level first: everything below depends on RELATIVE_PATH, which is the
    // whole reason this needs no permission.
    let sdk = {
        let class = env.find_class("android/os/Build$VERSION")?;
        env.get_static_field(class, "SDK_INT", "I")?.i()?
    };
    if sdk < API_Q {
        return Err(anyhow!(
            "Android {sdk} has no permissionless public Downloads; the file stays in the app"
        ));
    }

    let resolver = env
        .call_method(
            &activity,
            "getContentResolver",
            "()Landroid/content/ContentResolver;",
            &[],
        )?
        .l()?;

    // The row: a name, a type, and where it goes. `IS_PENDING` keeps it out of
    // everybody else's sight until the bytes are all there.
    let values = env.new_object("android/content/ContentValues", "()V", &[])?;
    put_string(&mut env, &values, "_display_name", name)?;
    put_string(&mut env, &values, "mime_type", &mime_for(name))?;
    put_string(&mut env, &values, "relative_path", RELATIVE_PATH)?;
    put_int(&mut env, &values, "is_pending", 1)?;

    let collection = {
        let class = env.find_class("android/provider/MediaStore$Downloads")?;
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
        return Err(anyhow!("MediaStore refused a row for {name}"));
    }

    // Copy through a descriptor rather than a Java stream: the bytes then move
    // in Rust, at Rust's pace, and JNI is left holding nothing but the handle.
    let copied = copy_into(&mut env, &resolver, &uri, staged);

    // Published or not, the row must stop being pending — a row left pending is
    // a file nobody can see and nothing will clean up promptly.
    let done = env.new_object("android/content/ContentValues", "()V", &[])?;
    put_int(&mut env, &done, "is_pending", 0)?;
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
            let s: String = env.get_string(&s.into())?.into();
            // The staged copy has served its purpose. Failing to remove it is
            // not worth failing the publish over — it is a file in the app's own
            // storage, and the next launch's sweep will find it.
            let _ = std::fs::remove_file(staged);
            Ok(SavedLocation::Uri(s))
        }
        Err(e) => {
            // Take the row back out: a half-written entry in somebody's
            // Downloads is worse than no entry at all.
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
            Err(e)
        }
    }
}

#[cfg(target_os = "android")]
/// Open the row for writing and pour the staged file into it.
fn copy_into(
    env: &mut jni::JNIEnv<'_>,
    resolver: &JObject<'_>,
    uri: &JObject<'_>,
    staged: &Path,
) -> Result<()> {
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
        return Err(anyhow!("no descriptor for the row just inserted"));
    }
    // `detachFd` hands ownership over: Java stops closing it, and the `File`
    // below closes it exactly once when it drops.
    let fd = env.call_method(&pfd, "detachFd", "()I", &[])?.i()?;
    let mut out = unsafe { std::fs::File::from_raw_fd(fd) };
    let mut input =
        std::fs::File::open(staged).with_context(|| format!("read {}", staged.display()))?;
    std::io::copy(&mut input, &mut out).context("copy into Downloads")?;
    // Flush before the pending flag clears, or another app can open a file whose
    // last block is still in a buffer.
    use std::io::Write as _;
    out.flush().context("flush into Downloads")?;
    Ok(())
}

#[cfg(target_os = "android")]
pub(crate) fn put_string(
    env: &mut jni::JNIEnv<'_>,
    values: &JObject<'_>,
    key: &str,
    v: &str,
) -> Result<()> {
    let k = env.new_string(key)?;
    let v = env.new_string(v)?;
    env.call_method(
        values,
        "put",
        "(Ljava/lang/String;Ljava/lang/String;)V",
        &[JValue::Object(&k), JValue::Object(&v)],
    )?;
    Ok(())
}

#[cfg(target_os = "android")]
pub(crate) fn put_int(
    env: &mut jni::JNIEnv<'_>,
    values: &JObject<'_>,
    key: &str,
    v: i32,
) -> Result<()> {
    let k = env.new_string(key)?;
    let boxed = env.new_object("java/lang/Integer", "(I)V", &[JValue::Int(v)])?;
    env.call_method(
        values,
        "put",
        "(Ljava/lang/String;Ljava/lang/Integer;)V",
        &[JValue::Object(&k), JValue::Object(&boxed)],
    )?;
    Ok(())
}

/// What the system should think this file is.
///
/// Compiled everywhere so its table — and the tests under it — are checked on
/// the machine this is written on; used only where there is a MediaStore to
/// tell.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
///
/// Only enough to make the common things open in the right app when tapped from
/// a file manager; anything unrecognised is a stream of bytes, which is true and
/// which every Android will offer a chooser for.
fn mime_for(name: &str) -> String {
    let ext = PathBuf::from(name)
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    match ext.as_str() {
        "pdf" => "application/pdf",
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "heic" => "image/heic",
        "mp4" | "m4v" => "video/mp4",
        "mov" => "video/quicktime",
        "mkv" => "video/x-matroska",
        "mp3" => "audio/mpeg",
        "m4a" => "audio/mp4",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "txt" | "md" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "json" => "application/json",
        "xml" => "application/xml",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "7z" => "application/x-7z-compressed",
        "epub" => "application/epub+zip",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::mime_for;

    /// The type is what decides whether tapping the file in a file manager opens
    /// a viewer or a chooser. Worth pinning the shapes that matter — and worth
    /// pinning the fallback, which must be a real type and not an empty string:
    /// MediaStore rejects a row with no `mime_type`.
    #[test]
    fn common_files_get_a_type_the_system_knows() {
        assert_eq!(mime_for("relazione.pdf"), "application/pdf");
        assert_eq!(mime_for("foto.JPG"), "image/jpeg");
        assert_eq!(mime_for("clip.mp4"), "video/mp4");
        assert_eq!(mime_for("note.txt"), "text/plain");
    }

    #[test]
    fn anything_else_is_bytes_rather_than_nothing() {
        assert_eq!(mime_for("dump.bin"), "application/octet-stream");
        assert_eq!(mime_for("senza-estensione"), "application/octet-stream");
        assert_eq!(mime_for(".profile"), "application/octet-stream");
    }
}
