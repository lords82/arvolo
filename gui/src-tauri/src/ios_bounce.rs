//! Giving the scroll back its ends.
//!
//! Pull a list past its last row on this platform and it comes back — the
//! rubber band is not decoration, it is how the system says "that is all there
//! is", and its absence is one of the two or three things that make a web view
//! read as a web view. Ours had no ends: a list simply stopped dead, top and
//! bottom.
//!
//! It was not the stylesheet, which is where two rounds of looking went. The
//! layer under Tauri turns it off for everyone, in as many words:
//!
//! ```text
//! // disable scroll bounce by default
//! scroll_view.setBounces(false)
//! ```
//!
//! — `wry/src/wkwebview/mod.rs`. A sensible default for a *window* whose content
//! is a fixed layout, and the wrong one for an app whose screens are lists. No
//! amount of `overscroll-behavior` reaches it: that property asks the web engine
//! for something the scroll view above it has already refused.
//!
//! So it is switched back on, once, on the webview this app actually has. The
//! horizontal axis stays locked (`alwaysBounceHorizontal` is left alone and the
//! content never exceeds the width), because a screen that can be pushed
//! sideways by a thumb is a screen that looks broken; only the vertical one is
//! wanted, and only where there is something to scroll.

#![cfg(target_os = "ios")]

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_ui_kit::UIScrollView;

/// Turn the rubber band back on for this window's webview.
///
/// Best-effort by design: a failure here costs the bounce and nothing else, and
/// there is no version of "the app will not start because a list does not spring
/// back" that is worth writing.
pub fn restore(window: &tauri::WebviewWindow) {
    let dispatched = window.with_webview(|w| {
        let webview = w.inner() as *mut AnyObject;
        if webview.is_null() {
            println!("arvolo: no webview to give its ends back to");
            return;
        }
        unsafe {
            let scroll: Option<Retained<UIScrollView>> = msg_send![webview, scrollView];
            let Some(scroll) = scroll else { return };
            scroll.setBounces(true);
            println!("arvolo: scroll bounce is on again");
            // Vertically always, so a *short* list bounces too. Without it the
            // ends only exist on screens long enough to scroll, and the same
            // gesture answering on one screen and not the next is worse than it
            // never answering at all.
            scroll.setAlwaysBounceVertical(true);
        }
    });
    if let Err(e) = dispatched {
        println!("arvolo: could not reach the webview: {e}");
    }
}
