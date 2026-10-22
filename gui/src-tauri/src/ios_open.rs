//! Handing a received file to the rest of the phone.
//!
//! "Open" on a desktop means asking the system to launch whatever owns the file
//! type, and `tauri-plugin-opener` does exactly that — through the `open` crate,
//! which shells out to `open` / `xdg-open` / `start`. None of those exist on iOS,
//! so the call succeeded, spawned nothing, and the button did nothing at all: a
//! `.txt` sat in Files while the row that had downloaded it offered to open it.
//!
//! iOS has no "open with" outside the share sheet — apps declare the types they
//! can take and the system offers them there. `UIActivityViewController` is that
//! sheet, and it answers both halves of what a person wants from a file that just
//! arrived (open it somewhere, or send it on) with one control they already know.
//!
//! Presented from the root view controller, which is the window Tauri put the
//! webview in. On iPad the sheet is a popover and refuses to appear without
//! somewhere to point, so it is given the middle of that view: the thing that
//! asked was a row inside a web view, and its rectangle is not knowable from this
//! side of the bridge.

#![cfg(target_os = "ios")]

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::MainThreadOnly;
use objc2_core_foundation::{CGPoint, CGRect, CGSize};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};
use objc2_ui_kit::{UIActivityViewController, UIApplication};

/// Offer `path` to the rest of the system. Returns as soon as the sheet is asked
/// for; presenting it is the main thread's business.
pub fn share(app: &tauri::AppHandle, path: &str) -> Result<(), String> {
    let path = path.to_string();
    app.run_on_main_thread(move || {
        if let Err(e) = present(&path) {
            // Nothing to answer: the caller returned before the main thread got
            // here. This lands in the device log, and it means the screen in
            // front of the person did not change — which is the failure this
            // whole file exists to stop being silent.
            eprintln!("arvolo: could not offer {path}: {e}");
        }
    })
    .map_err(|e| format!("{e}"))
}

/// Lo stesso foglio, ma con un indirizzo invece di un file (18c).
///
/// `NSURL::URLWithString` e non `fileURLWithPath`: la differenza non è di forma
/// — un `file://` offre «salva in File» e «apri con», un `https://` offre
/// Messaggi, Mail, WhatsApp. È la seconda che serve a un link pubblico, che
/// esiste per essere mandato a qualcuno.
pub fn share_url(app: &tauri::AppHandle, url: &str) -> Result<(), String> {
    let url = url.to_string();
    app.run_on_main_thread(move || {
        if let Err(e) = present_items(|| {
            let s = NSString::from_str(&url);
            NSURL::URLWithString(&s).map(|u| unsafe { Retained::cast_unchecked::<AnyObject>(u) })
        }) {
            eprintln!("arvolo: could not offer {url}: {e}");
        }
    })
    .map_err(|e| format!("{e}"))
}

fn present(path: &str) -> Result<(), String> {
    let path = path.to_string();
    present_items(|| {
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path));
        Some(unsafe { Retained::cast_unchecked::<AnyObject>(url) })
    })
}

fn present_items(item: impl FnOnce() -> Option<Retained<AnyObject>>) -> Result<(), String> {
    let mtm = MainThreadMarker::new().ok_or("not on the main thread")?;
    let ui = UIApplication::sharedApplication(mtm);

    // `windows` is deprecated in favour of asking a window scene, and this app has
    // exactly one — but reaching the scene means walking `connectedScenes` and
    // downcasting, for the same list. Deprecated is not gone, and the day it is
    // this fails to compile rather than at runtime.
    #[allow(deprecated)]
    let windows = ui.windows();
    let window = windows
        .iter()
        .find(|w| w.isKeyWindow())
        .or_else(|| windows.iter().next())
        .ok_or("this app has no window yet")?;
    let root = window
        .rootViewController()
        .ok_or("the window has no view controller")?;

    let one = item().ok_or("nothing to share")?;
    let items: Retained<NSArray<AnyObject>> = NSArray::from_retained_slice(&[one]);
    let sheet = unsafe {
        UIActivityViewController::initWithActivityItems_applicationActivities(
            UIActivityViewController::alloc(mtm),
            &items,
            None,
        )
    };

    {
        if let Some(pop) = sheet.popoverPresentationController() {
            let view = root.view().ok_or("the view controller has no view")?;
            let b = view.bounds();
            pop.setSourceView(Some(&view));
            pop.setSourceRect(CGRect::new(
                CGPoint::new(b.size.width / 2.0, b.size.height / 2.0),
                CGSize::new(1.0, 1.0),
            ));
        }
        root.presentViewController_animated_completion(&sheet, true, None);
    }
    Ok(())
}
