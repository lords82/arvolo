// The frame: rail, header, one view, and every overlay that can sit on top.
//
// Three things live here because they are properties of the *window* rather than
// of any screen: the drag-and-drop target (a file dropped anywhere means "send
// this", whatever is showing), the global keyboard map, and the two banners that
// report the app's own health — a daemon that cannot be reached, and a daemon
// whose version no longer matches this build.

import { useEffect, useMemo, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { enable as autostartEnable } from "@tauri-apps/plugin-autostart";
import { onArvoloTicket, onPickedFiles } from "./ipc";
import { api } from "./ipc";
import { fire, phoneTitle, TITLE_KEY, useStore, type Route } from "./store";
import { isBusy, shouldAnnounce } from "./update";
import { fmtBytes } from "./format";
import { useT } from "./i18n";
import { Icon } from "./ui/Icons";
import { Button, IconButton, modKey, TextInput } from "./ui/Primitives";
import { ToastHost, toast } from "./ui/Toasts";
import { Rail } from "./views/Rail";
import { TabBar } from "./views/TabBar";
import { CasellaView } from "./views/CasellaView";
import { CodesView } from "./views/CodesView";
import { YouView } from "./views/YouView";
import { AutoAcceptView } from "./views/AutoAcceptView";
import { ThreadSheet } from "./overlays/ThreadSheet";
import { useIsPhone } from "./phone";
import { tookBack } from "./backstack";
import { useEdgeBack } from "./edgeback";
import { TransfersView } from "./views/TransfersView";
import { PeopleBarMenu, PeopleView } from "./views/PeopleView";
import { DepositsView } from "./views/DepositsView";
import { HistoryView } from "./views/HistoryView";
import { DevicesView } from "./views/DevicesView";
import { SettingsView } from "./views/SettingsView";
import { SendSheet } from "./overlays/SendSheet";
import { ReceiveSheet } from "./overlays/ReceiveSheet";
import { IncomingDialog } from "./overlays/IncomingDialog";
import { ShareSheet } from "./overlays/ShareSheet";
import { PairSheet } from "./overlays/PairSheet";
import { CommandPalette } from "./overlays/CommandPalette";

/** Only the board is searchable in place; the other screens carry their own
 *  filter controls, and a header field that silently did nothing on four of six
 *  screens would be worse than no field. */
const SEARCHABLE: Route[] = ["transfers"];

/** Which screen a place is, on each platform.
 *
 *  A function rather than an expression inside the render because a push has to
 *  draw *two* screens for the length of the animation: the one arriving and the
 *  one being left behind. */
function viewFor(route: Route, isPhone: boolean) {
  return route === "transfers"
    ? isPhone
      ? CasellaView
      : TransfersView
    : route === "people"
      ? PeopleView
      : route === "codes"
        ? CodesView
        : route === "autoAccept"
          ? AutoAcceptView
          : route === "deposits"
            ? DepositsView
            : route === "history"
              ? HistoryView
              : route === "devices"
                ? DevicesView
                : // The desktop lands on its settings window; a phone lands on
                  // *You* and reaches the window from a row inside it.
                  route === "settings" && isPhone
                  ? YouView
                  : SettingsView;
}

export function App() {
  const isPhone = useIsPhone();
  // The phone class goes on the document element, not only on the app's own
  // div: sheets, menus and alerts render through portals into `document.body`,
  // and a phone style scoped under a div they are not inside is a style they
  // silently lose. On <html> it also sits beside `data-theme`, which makes the
  // dark-palette guard `.app-phone:not([data-theme="light"])` actually able to
  // see the choice it guards against.
  useEffect(() => {
    document.documentElement.classList.toggle("app-phone", isPhone);
    return () => document.documentElement.classList.remove("app-phone");
  }, [isPhone]);
  const stack = useStore((s) => s.stack);
  const back = useStore((s) => s.back);
  // Whether the title is still the big one under the bar, or has been handed up
  // into it. Reset on arrival: a new screen starts at the top of itself.
  const [bigTitle, setBigTitle] = useState(true);
  const viewRef = useRef<HTMLElement>(null);
  const t = useT();
  const init = useStore((s) => s.init);
  const connected = useStore((s) => s.connected);
  const status = useStore((s) => s.status);
  const guiVersion = useStore((s) => s.guiVersion);
  const loadError = useStore((s) => s.loadError);
  const actionError = useStore((s) => s.actionError);
  const dismissActionError = useStore((s) => s.dismissActionError);
  const reload = useStore((s) => s.reload);
  const route = useStore((s) => s.route);
  // The screen being left behind, for as long as it takes to leave it.
  //
  // Switching *tabs* sets nothing: this platform cross-cuts between roots with
  // no animation at all, and a slide there would claim a depth that is not
  // there. Only going in and coming out of a stack moves.
  const [leaving, setLeaving] = useState<{ route: Route; dir: "push" | "pop" } | null>(
    null
  );
  const seen = useRef({ route, depth: 1 });
  // A screen you have just arrived at starts at the top of itself.
  useEffect(() => setBigTitle(true), [route]);
  useEffect(() => {
    const was = seen.current;
    seen.current = { route, depth: stack.length };
    if (!isPhone || was.route === route) return;
    const dir =
      stack.length > was.depth ? "push" : stack.length < was.depth ? "pop" : null;
    if (!dir) return;
    setLeaving({ route: was.route, dir });
    // Slightly longer than the animation: taking the old screen away a frame
    // early is a flicker at the very end of the movement, which is the one part
    // of it people watch.
    const done = window.setTimeout(() => setLeaving(null), 380);
    return () => window.clearTimeout(done);
  }, [route, stack.length, isPhone]);
  // The way back a thumb actually uses.
  //
  // Two of them, for two platforms. On iOS the drag from the left edge is the
  // app's to implement — the system does not take it, and a stack without it is
  // one you have to climb by aiming at a corner. On Android that edge *is* the
  // system's back gesture, and it arrives here as a history pop, which is why
  // `push` leaves an entry behind for it to pop.
  useEdgeBack(viewRef, isPhone && stack.length > 1, () => window.history.back());
  useEffect(() => {
    // Layers first — a sheet, an alert, a menu, a thread — and only then the
    // stack. `backstack` holds what is open and knows which pops are its own.
    const onPop = () => {
      if (!tookBack()) back();
    };
    window.addEventListener("popstate", onPop);
    return () => window.removeEventListener("popstate", onPop);
  }, [back]);
  const go = useStore((s) => s.go);
  const search = useStore((s) => s.search);
  const setSearch = useStore((s) => s.setSearch);
  const openSheet = useStore((s) => s.openSheet);
  const openReceive = useStore((s) => s.openReceive);
  const openIncoming = useStore((s) => s.openIncoming);
  const restartDaemon = useStore((s) => s.restartDaemon);
  const setPaletteOpen = useStore((s) => s.setPaletteOpen);
  const clearFinished = useStore((s) => s.clearFinished);
  const transfers = useStore((s) => s.transfers);
  const update = useStore((s) => s.update);
  const startUpdate = useStore((s) => s.startUpdate);
  const dismissUpdate = useStore((s) => s.dismissUpdate);
  // Only true once the sync summary has been read, which `checkForUpdate` does
  // when it finds something. Unknown reads as "not paired": a warning about
  // other machines shown to somebody who has none is just noise.
  const paired = useStore((s) => s.sync?.paired ?? false);

  const [dragging, setDragging] = useState(false);

  // Select the stable map, derive the array with useMemo — a selector returning
  // a fresh array every call makes useSyncExternalStore loop forever.
  const pendingOffers = useMemo(
    () => Object.values(transfers).filter((t) => t.status === "incoming"),
    [transfers]
  );
  const finishedCount = useMemo(
    () =>
      Object.values(transfers).filter(
        (t) =>
          t.status === "completed" ||
          t.status === "cancelled" ||
          t.status === "failed"
      ).length,
    [transfers]
  );

  // The CLI refuses to talk to a mismatched daemon; the GUI shows a banner
  // instead (an old daemon reports version "" here).
  const versionMismatch =
    connected && status !== null && guiVersion !== "" && status.version !== guiVersion;

  // An update is announced only when the board is quiet: the check itself is a
  // GET and disturbs nobody, but proposing a restart in the middle of a send is
  // the wrong thing at the worst moment. It comes back on its own.
  const busy = useMemo(
    () => isBusy(Object.values(transfers).map((t) => t.status)),
    [transfers]
  );
  const announceUpdate = shouldAnnounce(update.info, update.dismissed, busy);

  // Boot the store (snapshot + event subscription) once. `init` is async, so an
  // unmount can land before it resolves — track that and dispose the listeners
  // it hands back, or they'd leak (and double up under StrictMode's re-mount).
  useEffect(() => {
    let disposed = false;
    let cleanup: (() => void) | undefined;
    init().then((c) => {
      if (disposed) c();
      else cleanup = c;
    });
    return () => {
      disposed = true;
      cleanup?.();
    };
  }, [init]);

  // Receive-first default: the very first launch registers the login entry, so
  // Arvolo is already there the day somebody sends a file. Applied exactly
  // once — the marker, not the OS state, is what makes this a default rather
  // than a rule: after it is set, the switch in Settings is the only thing
  // that touches autostart again, so "off" stays off. Not in dev, where the
  // entry would point at a debug binary.
  useEffect(() => {
    if (import.meta.env.DEV) return;
    try {
      if (localStorage.getItem("arvolo.autostartDefault")) return;
      localStorage.setItem("arvolo.autostartDefault", "applied");
    } catch {
      // A refusing localStorage cannot remember "once": better no default than
      // one that re-applies — and re-overrides the user — on every launch.
      return;
    }
    autostartEnable().catch(() => {
      // The platform said no (no autostart dir, an odd install). Not worth a
      // banner at first boot; the switch in Settings still works.
    });
  }, []);

  // Native feel: no webview context menu (right-click → Reload/Inspect), and in
  // production also swallow the reload / devtools keyboard shortcuts. Text inputs
  // keep normal keyboard editing (Cmd/Ctrl+C/V still work).
  useEffect(() => {
    const onCtx = (e: MouseEvent) => e.preventDefault();
    const onKey = (e: KeyboardEvent) => {
      if (import.meta.env.DEV) return; // keep hot-reload while developing
      const k = e.key.toLowerCase();
      const isReload = (e.metaKey || e.ctrlKey) && k === "r";
      const devtools =
        k === "f12" ||
        ((e.metaKey || e.ctrlKey) && e.shiftKey && (k === "i" || k === "c" || k === "j")) ||
        (e.metaKey && e.altKey && k === "i");
      if (isReload || devtools || k === "f5") e.preventDefault();
    };
    document.addEventListener("contextmenu", onCtx);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("contextmenu", onCtx);
      document.removeEventListener("keydown", onKey);
    };
  }, []);

  // App shortcuts. Deliberately skipped while focus is in a text field: ⌘N in
  // the middle of typing a note should not throw the note away.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const el = document.activeElement;
      const typing =
        el instanceof HTMLInputElement ||
        el instanceof HTMLTextAreaElement ||
        (el as HTMLElement | null)?.isContentEditable === true;
      const mod = e.metaKey || e.ctrlKey;
      const k = e.key.toLowerCase();

      if (mod && k === "k") {
        e.preventDefault();
        setPaletteOpen(true);
        return;
      }
      if (typing) return;
      if (mod && k === "n") {
        e.preventDefault();
        openSheet([]);
      } else if (mod && k === "v" && !e.shiftKey) {
        // ⌘V outside a field means "I have a code on the clipboard".
        e.preventDefault();
        openReceive();
      } else if (mod && k === ",") {
        e.preventDefault();
        go("settings");
      } else if (k === "/" ) {
        e.preventDefault();
        document.getElementById("app-search")?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [setPaletteOpen, openSheet, openReceive, go]);

  // Refuse the webview's own drag-and-drop default, everywhere in the document.
  //
  // Not a nicety: releasing a file on a page that does not cancel `drop` makes the
  // webview *navigate to it*, which replaces the entire app with a blank page and
  // no way back. It has to be the document rather than the drop zone, because the
  // gesture that breaks it is precisely the one that misses the zone.
  useEffect(() => {
    const stop = (e: DragEvent) => e.preventDefault();
    document.addEventListener("dragover", stop);
    document.addEventListener("drop", stop);
    return () => {
      document.removeEventListener("dragover", stop);
      document.removeEventListener("drop", stop);
    };
  }, []);

  // Drag and drop, window-wide. The webview's own event is used ONLY for the
  // hover visuals: the payload it carries on drop is ignored, because the drop
  // itself is handled on the Rust side of the window, which registers the paths
  // and emits `files://picked` with opaque ids (see `bridge::PickedFiles` — no
  // command accepts a filesystem path from this webview any more).
  useEffect(() => {
    let un: (() => void) | undefined;
    getCurrentWebview()
      .onDragDropEvent((e) => {
        setDragging(e.payload.type === "over" || e.payload.type === "enter");
      })
      .then((f) => {
        un = f;
      })
      .catch(() => {
        // No drag-drop in this webview build; the picker still works.
      });
    return () => un?.();
  }, []);

  // The registered result of a drop, from the window handler in Rust.
  useEffect(() => {
    let un: (() => void) | undefined;
    onPickedFiles((items) => {
      if (items.length) openSheet(items);
    })
      .then((f) => {
        un = f;
      })
      .catch(() => {});
    return () => un?.();
  }, [openSheet]);

  // A `.arvolo` handed to the app is something to RECEIVE: dropped on the
  // window or double-clicked while running (event), or double-clicked cold
  // (pending, held Rust-side until this asks). Either way the receive sheet
  // opens with the ticket already in place.
  const openReceiveWith = useStore((s) => s.openReceiveWith);
  useEffect(() => {
    let un: (() => void) | undefined;
    onArvoloTicket((ticket) => openReceiveWith(ticket))
      .then((f) => {
        un = f;
      })
      .catch(() => {});
    api
      .takePendingTicket()
      .then((t) => {
        if (t) openReceiveWith(t);
      })
      .catch(() => {});
    return () => un?.();
  }, [openReceiveWith]);

  // A refused action is surfaced as a toast rather than left in the store: it is
  // fired from click handlers that cannot await, so without this the button
  // simply looks broken. Errors never auto-dismiss (see Toasts).
  //
  // The raw daemon string stays (it is the truth), but the toast leads with what
  // to DO about it and carries the button that does it — the action affordance
  // existed in Toasts from the start and no caller ever used it.
  useEffect(() => {
    if (!actionError) return;
    const low = actionError.toLowerCase();
    let action: { label: string; run: () => void } | undefined;
    let hint = "";
    if (low.includes("no daemon") || low.includes("daemon")) {
      action = { label: t("app.restart"), run: () => fire(restartDaemon()) };
      hint = t("app.errHintDaemon");
    } else if (low.includes("relay")) {
      action = { label: t("app.errOpenSettings"), run: () => go("settings") };
      hint = t("app.errHintRelay");
    } else if (low.includes("password")) {
      hint = t("app.errHintPassword");
    }
    toast.bad(t("app.actionFailed"), hint ? `${hint} — ${actionError}` : actionError, action);
    dismissActionError();
  }, [actionError, dismissActionError, t, go, restartDaemon]);

  // The desktop keeps every screen it had. Where a phone wants a different one —
  // and Transfers is the case: two columns of files against one list of people —
  // it gets its own, and the desktop never sees it.
  const View = viewFor(route, isPhone);

  return (
    <div className={`app${isPhone ? " app-phone" : ""}`}>
      {isPhone ? null : <Rail />}

      <div className="main">
        {/* --- health banners ------------------------------------------ */}
        {/* A wrapper that is nothing on a desktop (`display: contents`) and, on
            a phone, the tray these hang from under the bar. They cannot be rows
            of the column any more: the column is one cell now, with the view
            filling it and the glass laid over the top. */}
        <div className="banners">
        {!connected && (
          <div className="banner bad" role="status">
            <Icon.Alert size={14} className="tone-bad" />
            <span className="grow">
              {t("app.disconnected")}
              <span className="t-xs t-mut" style={{ display: "block" }}>
                {t("app.disconnectedDetail")}
              </span>
            </span>
            <Button size="sm" onClick={() => fire(reload())}>
              {t("common.retry")}
            </Button>
          </div>
        )}
        {versionMismatch && (
          <div className="banner" role="status">
            <Icon.Alert size={14} className="tone-warn" />
            <span className="grow">
              {t(
                "app.versionMismatch",
                status?.version || t("app.versionUnknown"),
                guiVersion
              )}
            </span>
            <Button size="sm" onClick={() => fire(restartDaemon())}>
              {t("app.restart")}
            </Button>
          </div>
        )}
        {loadError && (
          <div className="banner bad" role="status">
            <Icon.Alert size={14} className="tone-bad" />
            <span className="grow">{loadError}</span>
            <Button size="sm" onClick={() => fire(restartDaemon())}>
              {t("app.restart")}
            </Button>
          </div>
        )}
        {/* Downloading takes over the banner rather than adding a second one:
            it is the same subject, one step further along. There is no cancel —
            the download is a few tens of megabytes and stopping it would leave
            nothing behind worth the button. */}
        {update.phase === "downloading" && (
          <div className="banner" role="status">
            <Icon.Refresh size={14} />
            <span className="grow">
              {update.progress?.total
                ? t(
                    "app.updateDownloading",
                    fmtBytes(update.progress.downloaded),
                    fmtBytes(update.progress.total)
                  )
                : t("app.updateDownloadingUnknown")}
            </span>
          </div>
        )}
        {update.phase !== "downloading" && announceUpdate && update.info && (
          <div className="banner" role="status">
            <Icon.Refresh size={14} />
            <span className="grow">
              {t("app.updateAvailable", update.info.version)}
              {/* Two second lines, and both are the kind of thing that belongs
                  next to the button rather than in release notes nobody opens.
                  The first says why the newest release is not the one on offer;
                  the second warns that updating one machine of a paired set and
                  not the others stops them syncing until they catch up. */}
              {update.info.heldBack && (
                <span className="t-xs t-mut" style={{ display: "block" }}>
                  {t(
                    "app.updateHeldBack",
                    update.info.heldBack.latest,
                    update.info.heldBack.relay ?? "?"
                  )}
                </span>
              )}
              {paired && (
                <span className="t-xs t-mut" style={{ display: "block" }}>
                  {t("app.updateDevices")}
                </span>
              )}
            </span>
            <Button size="sm" variant="in" onClick={() => startUpdate()}>
              {t("app.updateNow")}
            </Button>
            <IconButton
              label={t("app.updateLater")}
              onClick={() => dismissUpdate()}
            >
              <Icon.Close size={13} />
            </IconButton>
          </div>
        )}
        {pendingOffers.length > 0 && route !== "transfers" && (
          <div className="banner" role="status">
            <Icon.Receive size={14} className="tone-in" />
            <span className="grow">
              {pendingOffers.length === 1
                ? t("app.offerWaiting")
                : t("app.offersWaiting", pendingOffers.length)}
            </span>
            <Button
              size="sm"
              variant="in"
              onClick={() =>
                pendingOffers.length === 1
                  ? openIncoming(pendingOffers[0].offerId!)
                  : go("transfers")
              }
            >
              {t("app.seeOffers")}
            </Button>
          </div>
        )}
        </div>

        {/* --- header --------------------------------------------------- */}
        <header
          className={`header${
            isPhone ? ` nav${bigTitle && stack.length === 1 ? " big" : ""}` : ""
          }`}
        >
          {/* A back button, and the name of where it goes. Apple's own: the
              chevron alone is a guess, and the label is what turns a stack into
              somewhere you can read your way out of. */}
          {isPhone && stack.length > 1 && (
            <button className="nav-back" onClick={() => window.history.back()}>
              <Icon.ChevronLeft size={19} />
              <span className="truncate">
                {t(phoneTitle(stack[stack.length - 2]))}
              </span>
            </button>
          )}
          {/* The phone calls this place what the design calls it: the mailbox is
              the thing you open to see who has written, and on that screen the
              rows are people rather than files. */}
          <h1>{t(isPhone ? phoneTitle(route) : TITLE_KEY[route])}</h1>
          <div className="spacer" />

          {/* A 240px field is a third of a phone. The same search is a tap away
              as a glyph, and it searches everything rather than filtering one
              list. */}
          {SEARCHABLE.includes(route) && !isPhone && (
            <div style={{ width: 240 }}>
              <TextInput
                id="app-search"
                value={search}
                onChange={(e) => setSearch(e.currentTarget.value)}
                placeholder={t("app.searchPlaceholder")}
                aria-label={t("app.searchLabel")}
              />
            </div>
          )}

          {/* A desktop toolbar's leftover. On a phone the bar is a title and two
              glyphs; clearing what has finished is a thing you go and do, not
              something that stands between the title and the search. */}
          {!isPhone && route === "transfers" && finishedCount > 0 && (
            <Button size="sm" onClick={() => fire(clearFinished())}>
              <Icon.Trash size={13} />
              {t("app.clearFinished", finishedCount)}
            </Button>
          )}

          {/* Search, and a "+" that belongs to the screen it is on: on the
              address book it adds a person (a menu of the ways in — see
              `PeopleBarMenu`), everywhere else it starts a send. One glyph,
              the screen's own meaning, which is how this platform's bars
              work — Mail's compose and Contacts' add are the same corner. */}
          {isPhone && (
            <>
              <IconButton
                label={t("app.searchLabel")}
                onClick={() => setPaletteOpen(true)}
              >
                <Icon.Search size={17} />
              </IconButton>
              {route === "people" ? (
                <PeopleBarMenu />
              ) : (
                <IconButton label={t("app.send")} onClick={() => openSheet([])}>
                  <Icon.Plus size={17} />
                </IconButton>
              )}
            </>
          )}

          {/* Both are already in the bottom bar on a phone, and one of them is
              named after a key combination nobody can press there. */}
          {isPhone ? null : (
            <>
              <IconButton
                label={t("app.palette", modKey)}
                onClick={() => setPaletteOpen(true)}
              >
                <Icon.Search size={15} />
              </IconButton>
              <Button
                size="sm"
                variant="primary"
                onClick={() => openSheet([])}
                title={t("app.sendShortcut", modKey)}
              >
                <Icon.Send size={13} />
                {t("app.send")}
              </Button>
            </>
          )}
        </header>

        {/* --- the screen ----------------------------------------------- */}
        <main ref={viewRef} className={`view${isPhone ? " staged" : ""}`}>
          {leaving && (
            <div className={`page leaving ${leaving.dir}`} aria-hidden>
              {(() => {
                const Gone = viewFor(leaving.route, isPhone);
                return <Gone />;
              })()}
            </div>
          )}
          <div
            className={`page${leaving ? ` entering ${leaving.dir}` : ""}`}
            onScroll={
              // On the page, because the page is what scrolls here. The bar
              // takes the title over as the large one passes under the glass,
              // not at the first pixel of scroll: the two saying the same name
              // at once is the one state the handoff exists to prevent.
              isPhone && !leaving
                ? (e) => setBigTitle(e.currentTarget.scrollTop < 34)
                : undefined
            }
          >
            {isPhone && stack.length === 1 && (
              <h2 className="large-title">{t(phoneTitle(route))}</h2>
            )}
            <View />
          </div>
        </main>
      </div>

      {isPhone ? <TabBar /> : null}
      {isPhone ? <ThreadSheet /> : null}

      {/* --- overlays ---------------------------------------------------- */}
      <SendSheet />
      <ReceiveSheet />
      <IncomingDialog />
      <ShareSheet />
      <PairSheet />
      <CommandPalette />
      <ToastHost />

      {dragging && (
        <div className="drop-overlay">
          <div className="inner">
            <Icon.Send size={34} className="tone-out" />
            <div className="t-title">{t("app.dropTitle")}</div>
            <div className="t-sm t-mut">{t("app.dropHint")}</div>
          </div>
        </div>
      )}
    </div>
  );
}
