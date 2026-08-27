// The source dictionary. Every other language is type-checked against this one,
// so a missing key — or an interpolation that takes different arguments — is a
// compile error rather than a blank label in a shipped build.
//
// Entries are either a plain string or a function of exactly the values that
// have to be spliced into it. Functions rather than a `{name}` placeholder
// syntax on purpose: the argument list is then part of the type, so a call site
// cannot forget one and a translator cannot invent one.
//
// Keys are dotted by area. The prefix is where the string is *shown*, not where
// it happens to be constructed: `store.err.*` reads on screen as a toast, but it
// is raised from the store, and grouping it with the screens would scatter it.
//
// All of the above is about the *keys*. What goes in the values is `docs/COPY.md`
// — in short: say what happened, say what they can do, and put the reason on the
// hint line rather than in the label. The key suffix decides how long a value may
// be (`*Hint` and `*Body` are the second level and get room; `*.submit*` is a
// button and gets almost none), which is why the suffixes are worth keeping
// honest. `gui/src/test/copy.test.ts` enforces it and lists what is still over.

export const en = {
  // ---- locale ------------------------------------------------------------
  /** BCP 47 tag for `Intl`: weekday and month names, and clock convention. */
  "locale.tag": "en",
  /** This language's own name, for the settings picker. Never translated. */
  "locale.name": "English",

  // ---- shared ------------------------------------------------------------
  "common.cancel": "Cancel",
  "common.save": "Save",
  "common.done": "Done",
  "common.close": "Close",
  "common.confirm": "Confirm",
  "common.retry": "Retry",
  "common.open": "Open",
  "common.remove": "Remove",
  "common.refresh": "Refresh",
  "common.copy": "Copy",
  "common.copied": "Copied",
  "common.copyFailed": "Copy failed",
  "common.loading": "Loading…",
  "common.to": "to",
  "common.from": "from",
  /** Said by three separate settings toasts, two of them word for word. */
  "common.afterRestart": "It takes effect the next time the daemon starts.",

  // ---- app frame ---------------------------------------------------------
  "title.transfers": "Transfers",
  "title.people": "People",
  "title.deposits": "Links and deposits",
  "title.history": "History",
  "title.devices": "Your devices",
  "title.settings": "Settings",

  "app.disconnected": "Daemon unreachable",
  "app.disconnectedDetail": "Running transfers carry on; this window can't see them.",
  "app.versionMismatch": (daemon: string, gui: string) =>
    `Daemon ${daemon}, app ${gui} — restart it to line them up.`,
  "app.versionUnknown": "older",
  "app.restart": "Restart",
  "app.updateAvailable": (version: string) => `Arvolo ${version} is out.`,
  "app.updateNow": "Update",
  "app.updateLater": "Not now",
  "app.updateDownloading": (got: string, total: string) =>
    `Downloading the update — ${got} of ${total}.`,
  "app.updateDownloadingUnknown": "Downloading the update…",
  "app.updateHeldBack": (latest: string, relay: string) =>
    `${latest} is out, but ${relay} only carries this one.`,
  "app.updateDevices":
    "Update your other machines too, or they stop syncing.",
  "app.offerWaiting": "Someone wants to send you a file.",
  "app.offersWaiting": (n: number) => `${n} files waiting for you to confirm.`,
  "app.seeOffers": "See",
  "app.searchPlaceholder": "Filter by name or person…",
  "app.searchLabel": "Filter the transfers",
  "app.clearFinished": (n: number) => `Clear (${n})`,
  "app.palette": (mod: string) => `Search and run (${mod}K)`,
  "app.send": "Send",
  "app.sendShortcut": (mod: string) => `Send (${mod}N)`,
  "app.dropTitle": "Drop here to send",
  "app.dropHint": "Then choose who for: a contact, a code, a link.",
  "app.actionFailed": "That didn't work",
  "app.errHintDaemon": "The daemon isn't answering — restart it and try again.",
  "app.errHintRelay": "This needs a relay — set one under Settings → Network.",
  "app.errHintPassword": "This one is password-protected — ask the sender for the password.",
  "app.errOpenSettings": "Open Settings",

  "crash.title": "Something broke in the interface",
  "crash.body": "Transfers carry on in the daemon; you can pick up where you left off.",

  // ---- rail --------------------------------------------------------------
  "rail.nav": "Main navigation",
  "rail.meTitle": "Your identity and settings",
  "rail.meFallback": "Me",
  "rail.noIdentity": "identity not read yet",
  "rail.daemonUp": "Daemon connected",
  "rail.daemonDown": "Daemon unreachable",
  "rail.send": "Send…",
  "rail.receive": "Receive…",
  "rail.sections": "Sections",
  "rail.palette": "Search and run",

  // ---- transfer status ---------------------------------------------------
  "status.active": "Under way",
  "status.preparing": "Preparing…",
  "meta.preparing": "reading and encrypting — nothing sent yet",
  "status.sharing": "Shared",

  "share.title": "Shared file",
  "share.stop": "Stop sharing",
  "share.copies": "copies taken",
  "share.now": "downloading now",
  "share.lastPickup": "last taken",
  "share.never": "never",
  "share.uploaded": "uploaded",
  "share.fromDownload": (when: string) =>
    `Downloaded ${when}; your computer is now sharing it.`,
  "share.seedingSetting": "Change this",
  "share.countsNote": "Copies taken, not people: fetching twice counts twice.",
  "status.completed": "Completed",
  "status.deposited": "Deposited",
  "status.paused": "Paused",
  "status.incoming": "To confirm",
  "status.offered": "Offered",
  "status.stalled": "Stalled",
  "status.failed": "Failed",
  "status.cancelling": "Cancelling…",
  "status.cancelled": "Cancelled",

  "meta.paused": "paused",
  "meta.sharing": "available — nobody downloading it",
  "meta.sharingPeers": (n: number) =>
    n === 1 ? "1 person downloading it" : `${n} people downloading it`,
  "meta.offered": "waiting for them to see it",
  "meta.stalled": "resumes as soon as it can",
  // Why a send is waiting. One per `WaitingReason` in core/src/manager/mod.rs:
  // the daemon sends a code, not this sentence, precisely so this sentence can be
  // written in each language rather than arriving in English down a socket.
  // Shorter than the daemon's own sentences on purpose. Those are written for a
  // terminal, where a line has room to explain itself; this is the meta line
  // under a row, and the copy rules cap it at a glance's worth of words.
  "waiting.offered": "offered — waiting for them to accept",
  "waiting.tooLarge": "too big for the relay — will send direct",
  "waiting.relayRetry": (why: string) => `${why} — retrying, direct when they're online`,
  "waiting.parked": "parked on the relay — finishes without you",
  "waiting.relayCapped": (cap: string) => `relay caps this at ${cap} — direct from here`,
  "waiting.offloadFailed": (why: string) => `couldn't park it: ${why}`,
  "waiting.noRoute": "no relay reachable — local network only",
  // What goes where the relay's own words would, when it gave none.
  "waiting.relayUnnamed": "the relay could not take it",
  "meta.incoming": "open for the details",
  "meta.deposited": "waiting for the recipient to collect it",
  "meta.failed": "transfer failed",

  // ---- durations ---------------------------------------------------------
  // Short forms, for the ETA on a progress row.
  "eta.seconds": (n: number) => `${n} s`,
  "eta.minutes": (n: number) => `${n} min`,
  "eta.hours": (n: number) => `${n} h`,
  // Long forms, for a deadline that is days away.
  "until.seconds": (n: number) => (n === 1 ? "1 second" : `${n} seconds`),
  "until.minutes": (n: number) => (n === 1 ? "1 minute" : `${n} minutes`),
  "until.hours": (n: number) => (n === 1 ? "1 hour" : `${n} hours`),
  "until.days": (n: number) => (n === 1 ? "1 day" : `${n} days`),
  // Elapsed, for "last synced …".
  "ago.moments": "a few seconds ago",
  "ago.minutes": (n: number) => (n === 1 ? "1 minute ago" : `${n} minutes ago`),
  "ago.hours": (n: number) => (n === 1 ? "1 hour ago" : `${n} hours ago`),
  "ago.days": (n: number) => (n === 1 ? "1 day ago" : `${n} days ago`),

  // ---- board sections ----------------------------------------------------
  "section.pending": "To confirm",
  "section.active": "Under way and paused",
  "section.today": "Today",
  "section.earlier": "Earlier",

  // ---- transfers ---------------------------------------------------------
  "transfers.pause": "Pause",
  "transfers.resume": "Resume",
  "transfers.depositNow": "Leave it in their mailbox",
  "transfers.depositNowDesc":
    "Stop waiting for them to accept. The file goes to the relay and they collect it whenever.",
  "transfers.openFile": "Open file",
  "transfers.openFileFailed": "Couldn't open the file",
  "transfers.openFolder": "Open folder",
  "transfers.openFolderFailed": "Couldn't open the folder",
  "transfers.revokeDeposit": "Withdraw",
  "transfers.cancel": "Cancel",
  "transfers.removeRow": "Remove",
  "transfers.verifiedIdentity": "Verified identity",
  "transfers.swarm": "Transfer spread across several peers",
  "transfers.peers": (n: number) => (n === 1 ? "1 peer" : `${n} peers`),
  "transfers.liveCode": "code live",
  "transfers.review": "Review",
  "transfers.shareDetails": "Sharing details",
  "transfers.saveSender": "Save the sender…",
  "transfers.saveSenderTitle": "Save the sender to your contacts",
  "transfers.saveSenderSub": (name: string) => `They sent you ${name}.`,
  "transfers.reorder": (name: string) => `Move ${name}: drag or arrow keys`,
  "transfers.rowActions": (name: string) => `Actions for ${name}`,
  "transfers.progressOf": (name: string) => `Progress of ${name}`,
  "transfers.confirmRevokeTitle": "Withdraw the deposit?",
  "transfers.confirmCancelTitle": "Cancel?",
  "transfers.confirmRevokeBody": (peer: string) =>
    `The file is removed from the relay and the offer withdrawn from ${peer}'s mailbox. They won't be able to download it any more.`,
  "transfers.confirmRevokePeer": "the recipient",
  "transfers.confirmCancelBody": (name: string) =>
    `“${name}” stops here. Whatever has already gone across is thrown away: if you do it again, it starts from scratch.`,
  "transfers.confirmRevokeLabel": "Withdraw",
  "transfers.confirmCancelLabel": "Cancel the transfer",
  "transfers.keepGoing": "Leave it",
  "transfers.outgoing": "Outgoing",
  "transfers.incoming": "Incoming",
  "transfers.emptyOutTitle": "Nothing going out",
  "transfers.emptyInTitle": "Nothing coming in",
  "transfers.emptyOutBody": "Drag a file into the window, or use Send.",
  "transfers.emptyInBody": "Files people send you show up here.",
  "transfers.emptyOutAction": "Send something",
  "transfers.emptyInAction": "Paste a code",
  "transfers.firstRunTitle": "Drag the files you want to send here",
  // Not a list of the four modes: they are the four tabs of the send panel,
  // one click away and labelled.
  "transfers.firstRunBody": "Or use Send to pick a contact, a code, or a link.",
  "transfers.firstRunAction": "Send something",

  // ---- people ------------------------------------------------------------
  "people.presenceUnknownTitle": "The relay didn't answer",
  "people.presenceUnknownLabel": "Presence unknown",
  "people.presenceOnTitle": "Connected right now",
  "people.presenceOffTitle": "Not connected",
  "people.presenceOn": "Connected",
  "people.presenceOff": "Not connected",
  "people.lastExchange": (ago: string) => `last exchange ${ago}`,
  "people.listensOn": (relay: string) => `listens on ${relay}`,

  "people.menuDetails": "Details and fingerprint",
  "people.menuUnverify": "Unverify",
  "people.menuVerify": "Mark as verified…",
  "people.menuUntrust": "Stop trusting",
  "people.menuUntrustDesc": "You'll be asked for each file.",
  "people.menuTrust": "Trust",
  "people.menuTrustDesc": "Their files download without asking.",
  "people.menuUnblock": "Unblock",
  "people.menuBlock": "Block",
  "people.menuRemove": "Remove from contacts",
  "people.rowActions": (name: string) => `Actions for ${name}`,
  "people.notVerified": "Not verified",
  "people.notVerifiedTitle": "Fingerprint never compared",
  "people.claimedPlain": (name: string) => `(“${name}”)`,
  "people.claimedNew": (name: string) => `(calls themselves “${name}”)`,
  "people.claimedChange": (now: string, before: string) =>
    `(now: “${now}” — was: “${before}”)`,
  // A tooltip: it has to be readable at a glance or it is not one.
  "people.claimedTip": (mine: string) =>
    `Their own label, not proof. Yours — “${mine}” — never changes on its own.`,
  "people.claimedNote": (mine: string) =>
    `The name they now advertise. Yours for them stays “${mine}”.`,
  "people.acknowledge": "Got it",
  "people.send": "Send",
  "people.details": "Details",

  "people.confirmRemoveTitle": (name: string) => `Remove ${name}?`,
  "people.confirmRemoveBody":
    "The verified and trusted marks go with them. Past transfers stay in the history.",
  "people.confirmForceTitle": "Trust an unverified key?",
  // Floored in copy-rules.ts: it must say the fingerprint was never compared
  // *and* what that lets through. The footer is gone — it repeated the
  // conclusion the body had just reached.
  "people.confirmForceBody": (name: string) =>
    `You have never compared ${name}'s fingerprint. If somebody got in the middle when you added them, you would be auto-downloading from that person.`,
  "people.confirmForceLabel": "Force it anyway",
  "people.confirmForceCancel": "I'll verify first",

  "people.addTitle": "Add by id",
  "people.addSubtitle": "You need their public id in full.",
  "people.addNameLabel": "What you call them",
  "people.addNamePlaceholder": "e.g. Julia",
  "people.addIdLabel": "Public id",
  "people.addIdHint":
    "They find it with “arvolo me”, or on the Settings screen of their app.",
  // A pointer, not a second telling: `pair.subContactHost` is where swapping
  // codes is explained, on the sheet that does it.
  "people.addTip": "Swapping codes is quicker.",
  "people.addSaved": (name: string) => `Saved ${name}`,
  "people.addSavedDetail":
    "They stay unverified until you compare the fingerprint.",
  "people.addNameTaken": (name: string) =>
    `“${name}” already belongs to someone. Saving will ask whether to move the name.`,
  "people.confirmRekeyTitle": (name: string) => `Give “${name}” a new key?`,
  // Floored in copy-rules.ts: the marks being dropped is the whole consequence,
  // and a shorter wording turns an irreversible change into a confirmation.
  "people.confirmRekeyBody": (name: string) =>
    `“${name}” already points at another key. Moving the name drops their verified and trusted marks — the new key is unchecked until you compare it.`,
  "people.confirmRekeyCurrent": "The key it points at now",
  "people.confirmRekeyDo": "Move the name",
  "people.rekeyed": (name: string) => `“${name}” now points at the new key`,
  "people.rekeyedDetail": "Verified and trusted were dropped.",

  "person.fingerprint": "Fingerprint",
  // The canonical wording. Everywhere else that needs this either points here
  // or repeats it verbatim — nine near-variants was eight too many.
  "person.fingerprintHint":
    "The same words must appear on their screen. Compare them out loud, not over the channel you swapped ids on.",
  "person.publicId": "Public id",
  "person.verified": "Verified",
  "person.verifiedBody": "You have confirmed this fingerprint out of band.",
  "person.unverify": "Unverify",
  "person.notVerifiedYet": "Not verified yet",
  "person.notVerifiedBody":
    "All you know so far is that somebody gave you that id.",
  "person.compared": (name: string) =>
    `I compared the fingerprint with ${name} outside this app.`,
  "person.markVerified": "Mark as verified",
  "person.rename": "Rename",
  "person.renameHint": "The name is yours: the key and the marks stay put.",

  "people.swap": "Swap contacts",
  "people.haveCode": "Enter a code",
  "people.byId": "By id",
  "people.export": "Export",
  "people.import": "Import",
  "people.whoIsOnline": "Who's about",
  "people.whoIsOnlineTitle": "Ask the relay who is connected",
  "people.filterLabel": "Contact filter",
  "people.filterAll": "All",
  "people.filterVerified": "Verified",
  "people.filterTrusted": "Trusted",
  "people.filterBlocked": "Blocked",
  "people.filterBlockedN": (n: number) => `Blocked (${n})`,
  "people.searchPlaceholder": "Search by name or id…",
  "people.searchLabel": "Search contacts",
  "people.emptyNone": "Nobody in the address book",
  "people.emptyNoMatch": "No contact matches",
  "people.emptyNoneBody":
    "Swapping codes is quickest: you save each other, already verified.",
  "people.emptyNoMatchBody": "Try a different filter or search.",
  "people.exportFilename": "arvolo-contacts.json",
  "people.exportedOne": "Exported 1 contact",
  "people.exportedMany": (n: number) => `Exported ${n} contacts`,
  "people.exportDetail": "The file holds public ids only: no secrets.",
  "people.exportFailed": "Export failed",
  "people.importedOne": "Imported 1 contact",
  "people.importedMany": (n: number) => `Imported ${n} contacts`,
  "people.importDetail": (skipped: number) =>
    `${skipped ? `${skipped} skipped. ` : ""}All unverified: the marks are not imported, because those fingerprints are not ones you checked.`,
  "people.importFailed": "Import failed",
  "people.importNotAList": "the file is not a list",

  "trust.blocked": "Blocked",
  "trust.blockedTitle": "Their offers are dropped on arrival",
  "trust.verified": "Verified",
  "trust.verifiedTitle": "Fingerprint confirmed out of band",
  "trust.trusted": "Trusted",
  "trust.trustedTitle": "Their files download without asking",

  // ---- deposits ----------------------------------------------------------
  "deposit.expired": "Expired",
  "deposit.expiredDetail": "the deadline has passed",
  "deposit.taken": "Taken",
  "deposit.takenDetail": "the recipient fetched it",
  "deposit.offerPending": "it hasn't reached them yet",
  "deposit.offerArrived": "on their device, not taken yet",
  "deposit.gone": "No longer available",
  "deposit.goneLink": "downloaded up to the limit, or already withdrawn",
  "deposit.goneSealed": "collected by the recipient, or already withdrawn",
  "deposit.expiresIn": (until: string) => `expires in ${until}`,
  "deposit.expiredJustNow": "deadline just passed",
  "deposit.unknown": "State unknown",
  "deposit.unknownDetail": (when: string) => `relay unreachable · ${when}`,
  "deposit.downloads": (n: number, cap: string) => `${n}${cap} downloads`,
  "deposit.noLimit": "no limit",
  "deposit.max": (label: string) => `max ${label}`,
  "deposit.active": "Live",

  "deposits.openInBrowser": "Open in the browser",
  "deposits.openFailed": "Couldn't open the link",
  "deposits.share": "Link",
  "deposits.shareTicket": "Pickup code",
  "deposits.shareTicketTitle": "The ticket, again",
  "deposits.ticketDetail":
    "Paste it to the recipient. Only they can decrypt it — it is sealed to their identity.",
  "deposits.shareTitle": "The link, again",
  "deposits.publicLink": "Public link",
  "deposits.sealed": "Deposit",
  "deposits.revoke": "Withdraw",
  "deposits.sealedFor": (who: string, detail: string) =>
    `sealed for ${who} · ${detail}`,
  "deposits.confirmRevokeTitle": "Withdraw it?",
  "deposits.confirmRemoveTitle": "Remove the row?",
  "deposits.confirmRevokeLink":
    "The link stops working for everyone. Copies already downloaded stay downloaded.",
  "deposits.confirmRevokeSealed":
    "It comes off the relay and out of their mailbox. If they have not collected it, they cannot.",
  "deposits.confirmRemoveBody":
    "There is nothing left on the relay to take away: only this row goes.",
  "deposits.intro": "What you have left on a relay and can still take back.",
  "deposits.createLink": "Create a link",
  "deposits.emptyTitle": "No live link or deposit",
  "deposits.emptyBody": "Public links and mailbox deposits show up here.",
  "deposits.sectionLinks": "Public links",
  "deposits.sectionSealed": "Sealed deposits",

  // ---- history -----------------------------------------------------------
  "history.today": "Today",
  "history.yesterday": "Yesterday",
  "history.completed": "Completed",
  "history.cancelled": "Cancelled",
  "history.deposited": "Deposited",
  "history.failed": "Failed",
  "history.unknownOutcome": "Outcome unknown",
  "history.filterLabel": "History filter",
  "history.filterAll": "Everything",
  "history.filterSent": "Sent",
  "history.filterReceived": "Received",
  "history.searchPlaceholder": "Search…",
  "history.searchLabel": "Search the history",
  "history.clear": "Empty it",
  "history.emptyNoMatch": "No results",
  "history.emptyNothing": "Nothing yet",
  "history.emptyNoMatchBody": "Try a different filter or search.",
  "history.emptyNothingBody":
    "Every finished transfer ends up here: what, with whom, and how it went.",
  "history.confirmClearTitle": "Empty the history?",
  "history.confirmClearBody":
    "The log is forgotten in full and cannot be recovered. Files you have already received stay where they are; this only deletes the list.",

  // ---- devices -----------------------------------------------------------
  "devices.identityTitle": "Your shared identity",
  "devices.identityHint":
    "Every linked device uses this one. To everyone else you are one person.",
  "devices.fingerprint": "Fingerprint",
  "devices.fingerprintHint":
    "It must match on all your devices. Different words mean a different identity.",
  "devices.publicId": "Public id",
  "devices.sendToSelf": "Send to your devices",
  "devices.pairTitle": "Link a device",
  // How it is done. What it *costs* is in `pair.deviceWarnRest`, on the sheet
  // where the code appears — the moment somebody is about to read one out.
  "devices.pairBody":
    "You show a code here and type it in on the other machine.",
  "devices.showCode": "Show a code",
  "devices.haveCode": "Enter a code",
  "devices.pairWarnLead": "Never on a machine that isn't yours.",
  "devices.syncTitle": "Address book in step",
  "devices.syncHint":
    "Contacts travel inside an encrypted cell on your mailbox.",
  "devices.syncOn": "On",
  "devices.syncOff": "Off",
  "devices.contactCount": (n: number) =>
    n === 1 ? "1 contact in the address book" : `${n} contacts in the address book`,
  "devices.lastSync": (when: string) => `last synced ${when}`,
  "devices.neverSynced": "not synced since the daemon started",
  "devices.lastError": (err: string) => `The last round failed: ${err}`,
  "devices.syncNow": "Sync now",
  "devices.listTitle": "The machines",
  "devices.listHint":
    "Each one adds itself when it syncs. Self-reported: nothing here proves a row is a computer that still exists.",
  "devices.listEmpty": "No machine has announced itself yet.",
  "devices.thisDevice": "this device",
  "devices.neverSeen": "never synced",
  "devices.seenAgo": (when: string) => `seen ${when}`,
  "devices.rowActions": (name: string) => `Actions for ${name}`,
  "devices.rename": "Rename",
  "devices.renameTitle": "Name this device",
  "devices.renameHint":
    "Only this one: a device is described by itself, so the others are renamed on themselves.",
  "devices.renameThereDesc": "On that machine: it writes its own row.",
  "devices.forget": "Forget",
  "devices.forgetHereDesc":
    "You are on it — it would announce itself again on the next round.",
  "devices.confirmForgetTitle": (name: string) => `Forget ${name}?`,
  "devices.confirmForgetBody":
    "It leaves the list on every machine. It keeps the identity — nothing takes that back — but if it is running it stops announcing itself.",
  "devices.renamed": "Renamed",
  "devices.renamedDetail": "Your other machines see it after the next sync.",
  "devices.forgotten": "Forgotten",
  "send.deviceLabel": "Which of them",
  "send.deviceAny": "Whichever is awake",
  "send.deviceAnyHint": "Whichever of your machines is awake takes it.",
  "send.deviceOneHint":
    "The others leave it alone. If that machine is asleep, it waits on the relay until it wakes.",
  "store.renameDevice": "rename this device",
  "store.forgetDevice": "forget that device",
  "devices.autoTitle": "Sync on its own",
  "devices.autoDesc":
    "A round every few minutes. Off, it only lines up when you press the button.",
  "devices.autoOn": "Automatic sync on",
  "devices.autoOff": "Automatic sync off",

  // ---- settings ----------------------------------------------------------
  "settings.sourceEnv": "set by the ARVOLO_RELAY variable",
  "settings.sourceConfig": "saved in the settings",
  "settings.sourceBuiltin": "default, bundled with the app",
  "settings.sourceNone": "none",
  "settings.nameSaved": "Name updated",
  "settings.nameSavedDetail":
    "It travels inside every offer you send, from now on.",
  "settings.relaySaved": "Relay saved",
  "settings.relaySavedDetail": "Restart the daemon below to apply it now.",
  "settings.whoYouAre": "Who you are",
  "settings.nameLabel": "The name you show",
  // Not "and the fingerprint is what identifies you" — that is the next field
  // down, and it says so itself.
  "settings.nameHint":
    "A label you pick yourself: whoever receives it sees it in quotes.",
  "settings.namePlaceholder": "none",
  "settings.fingerprintLabel": "Your fingerprint",
  "settings.fingerprintHint":
    "The words others compare to be sure it is you. Read them out loud when somebody adds you.",
  "settings.publicIdLabel": "Your public id",
  "settings.appearance": "Appearance",
  "settings.theme": "Theme",
  "settings.themeSystem": "System",
  "settings.themeLight": "Light",
  "settings.themeDark": "Dark",
  "settings.language": "Language",
  "settings.languageAuto": "System",
  "settings.languageHint":
    "“System” follows your computer, falling back to English.",
  "settings.startup": "Startup",
  "settings.autostartTitle": "Start at login",
  "settings.autostartDesc":
    "Starts hidden in the tray, ready to receive. On Linux, switch it off and on again if you move the AppImage.",
  "settings.autostartFailed": "Could not change the login setting",
  "settings.network": "Network",
  "settings.relayOn": "Relay live",
  "settings.relayOff": "No relay",
  "settings.relayLabel": "Relay",
  "settings.relayLocked":
    "ARVOLO_RELAY is set, so what you type here has no effect.",
  "settings.relayHint": (current: string, source: string) =>
    `In use now: ${current} — ${source}. No scheme means https://; write http:// out in full.`,
  "settings.relayNone": "none",
  "settings.relayPlaceholder": "relay.example.com",
  // The one place this is said. Four other screens used to repeat it; this is
  // where the relay is chosen, so this is where it belongs.
  "settings.relayNote":
    "Routes the codes, the mailbox and the links. It never sees your files in the clear.",
  "settings.files": "Files",
  "settings.downloadDirLabel": "Where files land",
  "settings.downloadDirEnv": "Decided by the ARVOLO_DOWNLOAD_DIR variable.",
  "settings.downloadDirHint":
    "It applies to whatever you accept without picking a folder on the spot.",
  "settings.change": "Change",
  "settings.dirUpdated": "Folder updated",
  "settings.cannotOpen": "Couldn't open it",
  "settings.seedTitle": "Keep sharing what you downloaded",
  "settings.seedDesc": "It helps whoever is downloading the same file.",
  "settings.saved": "Setting saved",
  "settings.advanced": "Advanced",
  "settings.configFileLabel": "Configuration file",
  "settings.configFileHint":
    "Temp folder, NAT relay, log level: set by hand here, commented line by line.",
  "settings.identityKeyLabel": "Identity key",
  // Has a floor in copy-rules.ts: it must keep saying that the key *is* the
  // identity, not merely that it is private.
  "settings.identityKeyHint":
    "Your secret: whoever holds it becomes you. To use another machine, link the device.",
  "settings.versions": (daemon: string, gui: string) =>
    `Daemon ${daemon} · interface ${gui}`,
  "settings.daemonProcess": (pid: number, exe: string) => `pid ${pid} · ${exe}`,
  "settings.restartDaemon": "Restart the daemon",
  "settings.updatesTitle": "Updates",
  "settings.checkUpdates": "Check now",
  "settings.updateIdle": "Checked about once a week.",
  "settings.updateChecking": "Checking…",
  "settings.updateNone": "You're on the latest version.",
  "settings.updateFound": (version: string) => `Arvolo ${version} is out.`,
  "settings.updateInstalling": "Installing — the app restarts itself.",
  "settings.updateReopen": "Installed — open Arvolo again to finish.",
  "settings.updateFailed": (why: string) => `Couldn't check: ${why}`,
  "settings.confirmUpdateTitle": "Update while transfers are running?",
  "settings.confirmUpdateBody":
    "The daemon stops so the installer can replace it. Transfers in flight break off and pick up again when the app returns.",
  "settings.autoUpdateTitle": "Check automatically",
  "settings.autoUpdateDesc":
    "Once a week, asks github.com if there's a new version — the only third party Arvolo talks to. Nothing installs itself.",
  "settings.confirmRestartTitle": "Restart the daemon?",
  "settings.confirmRestartBody":
    "Transfers under way stop. Resumable ones pick up where they were; the rest start over.",
  "settings.restarting": "Daemon restarting",
  "settings.restartingDetail": "It comes back on its own in a few seconds.",
  "settings.refreshing": "Refreshing…",

  // ---- send sheet --------------------------------------------------------
  "send.modeContact": "To a contact",
  "send.modeSelf": "Your devices",
  "send.modeCode": "Code",
  "send.modeLink": "Link",
  "send.modeTicket": "Ticket",
  // Each mode gets a line saying what the *recipient* needs — the only thing
  // that decides which one is right — and a second line for what it costs.
  "send.blurbContact": "Straight to somebody in your address book.",
  "send.blurbContactMore":
    "Direct if they are online; otherwise it waits in their mailbox.",
  "send.blurbSelf": "The other machines that hold your identity.",
  "send.blurbSelfMore": "Nobody has to accept it.",
  "send.blurbCode": "A short code to read out or scan.",
  "send.blurbCodeMore":
    "They need not be a contact, but you both have to be online right now.",
  "send.blurbLink": "An address that opens in any browser.",
  "send.blurbLinkMore": "Whoever gets it needs no account and nothing installed.",
  "send.blurbTicket": "An arvc… ticket, straight from this machine.",
  "send.blurbTicketMore":
    "Neither the mailbox nor the Arvolo relay is involved.",
  "send.ttl1h": "1 hour",
  "send.ttl1d": "1 day",
  "send.ttl7d": "7 days",
  "send.ttl30d": "30 days",
  "send.pickerEmptyTitle": "Your address book is empty",
  "send.pickerEmpty":
    "Add somebody from People. Swapping codes is quickest — it saves you both, already verified.",
  "send.pickerSearch": "Search a contact…",
  "send.pickerRecipient": "Recipient",
  // The machines that share this identity. Never "me": the destination is a set
  // of computers, and one of them is the one being typed on.
  "send.selfUnpairedHint":
    "No other machine holds this identity yet — link one under Devices. What you send now waits on the relay.",
  "send.selfDepositHint":
    "A deposit is collected once: at one pickup, the first machine awake takes it and the others never see it.",
  "send.pickerNoMatch": (q: string) => `No contact matches “${q}”.`,
  "send.depositResult": (to: string) => `Deposited for ${to}`,
  // Why the ticket is on screen at all when nobody has to do anything with it.
  "send.depositResultDetail": (to: string) =>
    `Your own copy, in case ${to} never gets the offer.`,
  "send.onItsWay": (to: string) => `On its way to ${to}`,
  // Written whole rather than splicing a name in: "On its way to Your
  // devices" capitalises mid-sentence, and the other three languages come out
  // ungrammatical rather than merely odd.
  "send.onItsWaySelf": "On its way to your devices",
  "send.depositResultSelf": "Deposited for your devices",
  "send.depositResultSelfDetail":
    "The ticket, in case none of your machines sees the offer.",
  "send.saveArvolo": "Save as .arvolo file…",
  "send.arvoloSaved": (name: string) => `Saved ${name}`,
  "send.arvoloSavedDetail": "Share it over any channel; they open it with Arvolo.",
  "send.onItsWayDetail":
    "If they are online it goes direct, otherwise it waits in their mailbox.",
  "send.onItsWaySelfDetail":
    "It waits on the relay until one of your machines is awake to take it.",
  "send.codeKeepDetail":
    "Good for several recipients, until you cancel the send.",
  "send.codeOnceDetail": "Good for one recipient, then it retires.",
  "send.linkDetail":
    "Anyone with the address can download it until it expires, runs out, or you withdraw it.",
  "send.ticketDetail":
    "Good for as long as the daemon runs and the send is not cancelled.",
  "send.countOne": "1 item",
  "send.countMany": (n: number) => `${n} items · packed into one archive`,
  "send.titleReady": "Ready",
  "send.title": "Send",
  "send.subtitle": "Encrypted end to end, always.",
  "send.submitDeposit": "Deposit",
  "send.submitSend": "Send",
  "send.submitCode": "Generate code",
  "send.submitLink": "Create link",
  "send.submitTicket": "Create ticket",
  // The one place the `#` is explained, and the reason `send.blurbLink` above
  // does not mention it: this is where the link actually exists.
  "send.linkKeyNote":
    "The key rides after the `#`, which browsers never send to the server.",
  "send.filesLabel": "What you are sending",
  "send.filesHint": "You can also drag files and folders into the window.",
  "send.filesRemove": (name: string) => `Remove ${name}`,
  "send.pickFiles": "Files…",
  "send.pickFolder": "Folder…",
  "send.whoLabel": "Who it goes to",
  "send.modeLabel": "Way of sending",
  "send.noteLabel": "A couple of lines for the recipient (optional)",
  "send.noteHint": "It travels inside the sealed offer.",
  "send.notePlaceholder": "Here are the files we talked about.",
  "send.keepCodeTitle": "Good for several people",
  "send.keepCodeDesc":
    "Otherwise the code retires after one recipient.",
  "send.keepCodeLabel": "Code for several people",
  "send.depositTitle": "Deposit and don't wait",
  "send.depositDesc":
    "Leaves it on the relay even if they are online, so you can close the window. Unlocks expiry, collections and password.",
  "send.depositLabel": "Leave it in the mailbox",
  "send.expiresAfter": "Expires after",
  "send.depositTtlLabel": "Deposit expiry",
  "send.linkTtlLabel": "Link expiry",
  "send.maxPickupsLabel": "Collections allowed",
  "send.maxPickupsHint":
    "One, normally: the moment they download it, the relay deletes it.",
  "send.passwordLabel": "Password (optional)",
  // The sender's half of the password story: you choose it, and nobody can get
  // it back for you. The recipient's half lives in `incoming.passwordHint`.
  "send.passwordHint":
    "Nobody can recover it, the relay included. Lose it and the file is lost.",
  "send.passwordPlaceholder": "none",
  "send.linkTooMany":
    "A link publishes one item. Pick one, or put them in a folder.",
  "send.maxDownloadsLabel": "Downloads allowed",
  "send.maxDownloadsHint": "Leave it empty for no limit.",
  "send.maxDownloadsPlaceholder": "unlimited",
  "send.noRelay": "This way of sending needs a relay. Set one up in Settings.",
  "send.noArvoloRelay": "No Arvolo relay",

  // ---- receive sheet -----------------------------------------------------
  "receive.explainEmpty":
    "A send code (like 4821-crater-mango) or an arvc… / arvm… ticket.",
  "receive.explainCode": "Send code: downloads from whoever is showing it now.",
  "receive.explainChunk": "Peer-to-peer ticket: straight from the sender.",
  "receive.explainMailbox": "Mailbox ticket: collected from the relay.",
  "receive.explainUnknown":
    "Unfamiliar shape — worth trying, but check you copied it whole.",
  "receive.title": "Receive",
  "receive.subtitle": "Paste what you were given.",
  "receive.submit": "Receive",
  "receive.fieldLabel": "Code or ticket",
  "receive.passwordLabel": "Password, if protected",
  // The recipient's half of the password story; the sender's is in `send.*`.
  "receive.passwordHint": "The sender will have told you separately.",
  "receive.passwordPlaceholder": "none",
  "receive.whereLabel": "Where to save it",
  "receive.whereHint": (dir: string) => `Default folder: ${dir}`,
  "receive.whereAria": "Destination folder",
  "receive.choose": "Choose…",
  "receive.useDefault": "Default",
  "receive.started": "Receiving started",
  "receive.startedDetail": "You'll find it under incoming transfers.",

  // ---- incoming dialog ---------------------------------------------------
  "incoming.unknownSender": "Unknown sender",
  "incoming.started": "Receiving started",
  "incoming.title": "Somebody is sending you a file",
  "incoming.subtitle": "Only accept if you know who it is from.",
  "incoming.reject": "Reject",
  "incoming.later": "Later",
  "incoming.accept": "Accept",
  "incoming.notInBook": "Not in the address book",
  "incoming.claimedName": (name: string) => `goes by “${name}” — unverified`,
  "incoming.keyFingerprint": "Fingerprint of the key",
  "incoming.senderId": "Sender's public id",
  "incoming.hintVerified": "Already compared: the same key you verified.",
  "incoming.hintKnown":
    "Compare it out loud with the sender. A name does not prove who they are.",
  // Floored in copy-rules.ts: it must say that nobody has checked these words
  // yet, or the reader takes their presence for a check already made.
  "incoming.hintStranger":
    "Nobody has checked these words yet. Read them out to the sender over something other than this app.",
  // Floored too, for the reason the words are missing: an id is not evidence.
  "incoming.hintRawId":
    "Not a fingerprint — the raw id, because this daemon is too old to send the words. Update it to be able to compare.",
  // Floored: a first arrival is the one case where there is nothing at all to
  // check against, and saying less than that would hide it.
  "incoming.firstTime":
    "First time this key has sent you anything — there is nothing yet to compare it with.",
  "incoming.seenBefore": (n: string) =>
    `You have taken ${n} transfer(s) from this key before.`,
  "incoming.wasCalled": (name: string) => `Used to call itself “${name}”`,
  "incoming.attachedNote": "Message attached",
  "incoming.passwordLabel": "Password",
  "incoming.passwordHint": "The sender will have told you separately.",
  "incoming.ifYouKnowThem": "If you know them",
  "incoming.saveAsPlaceholder": "Save to the address book as…",
  "incoming.saveAsLabel": "Name to give the contact",
  "incoming.saveAsSuggested":
    "Proposed from the name they gave themselves. Change it: the name is yours.",
  "incoming.savedAs": (name: string) => `Saved as ${name}`,
  "incoming.savedAsDetail": "Still unverified until you compare the fingerprint.",
  "incoming.savedInline": (name: string) => `Saved as “${name}”, not verified`,
  "incoming.saveNote": "Saving is not verifying.",
  "incoming.acceptAndSave": "Accept and save",
  // Floored: it has to name whose marks would go, or "already taken" reads as a
  // tidiness rule rather than the reason it exists.
  "incoming.nameTaken": (name: string) =>
    `“${name}” is already someone else here. Re-pointing it would drop their verified and trusted marks.`,
  "incoming.blockAndReject": "Block and reject",
  "incoming.blocked": "Blocked",
  "incoming.blockedDetail": "Their offers are dropped on arrival.",

  // ---- pairing sheet -----------------------------------------------------
  "pair.titleContact": "Swap contacts",
  "pair.titleDeviceHost": "Link another device of yours",
  "pair.titleDeviceJoin": "Link this device",
  "pair.subContactHost":
    "Show them the code: you save each other, already verified.",
  "pair.subContactJoin": "Type in the code they read out to you.",
  "pair.subDeviceHost": "It shares your identity with the new machine.",
  "pair.subDeviceJoin":
    "It replaces this device's identity with the shared one.",
  "pair.restarting": "Daemon restarting",
  "pair.restartingDetail": "Back in a few seconds, with the shared identity.",
  "pair.restartAndClose": "Restart and close",
  "pair.link": "Link",
  "pair.done": "Done",
  "pair.needsRestart":
    "The daemon still has the previous identity. Restart it below.",
  "pair.failed": "That didn't work",
  "pair.deviceWarnLead": "This shares your secret identity.",
  // Has a floor in copy-rules.ts. It must keep saying that the person typing
  // the code *becomes* you — "gets access" is a different, smaller claim.
  "pair.deviceWarnRest":
    "Whoever types it becomes you: same id, same mailbox, same address book. Only on a machine of your own.",
  "pair.captionDevice": "On the other device: Your devices → Enter a code.",
  "pair.captionContact": "Read it out. They open People → Enter a code.",
  "pair.waitingOther": "Waiting for the other side… close to cancel.",
  "pair.preparingCode": "Preparing the code…",
  "pair.contactNote": "Only public ids are exchanged.",
  "pair.joinWarnLead": "Careful: this cannot be undone.",
  // Floored too: "replaced" and "stops opening" are the whole warning.
  "pair.joinWarnRest":
    "This device's identity is replaced by the shared one. Anything sealed to the old one stops opening here.",
  "pair.codeLabel": "Code",
  "pair.codeHint":
    "The one shown on the other machine, something like 4821-crater-mango.",
  "pair.nameLabel": "Your name for them",
  "pair.nameHint":
    "Leave it empty for a name derived from their fingerprint. You can rename them later.",
  "pair.understood": "I understand: this device loses its identity.",
  "pair.waitingMachine": "Waiting for the other machine…",
  "pair.cancelled": "Cancelled.",

  // ---- command palette ---------------------------------------------------
  "palette.groupGoTo": "Go to",
  "palette.groupActions": "Actions",
  "palette.groupPeople": "People",
  "palette.send": "Send files…",
  "palette.sendHint": "contact, code, link or ticket",
  "palette.sendKw": "send upload new share",
  "palette.receive": "Receive…",
  "palette.receiveHint": "paste a code or a ticket",
  "palette.receiveKw": "download get paste",
  "palette.pairContact": "Swap contacts",
  "palette.pairContactHint": "you save each other, already verified",
  "palette.pairContactKw": "pairing pair add person verify",
  "palette.pairDevice": "Link another device of yours",
  "palette.pairDeviceKw": "multidevice identity sync",
  "palette.sync": "Sync contacts now",
  "palette.syncKw": "contacts devices",
  "palette.resumeAll": "Resume every transfer",
  "palette.pauseAll": "Pause everything",
  "palette.pauseAllKw": "pause all stop hold resume",
  "palette.clearFinished": "Clear finished",
  "palette.clearFinishedKw": "empty completed tidy",
  "palette.navTransfersKw": "board sends",
  "palette.navPeopleKw": "contacts address book",
  "palette.navDepositsKw": "relay withdraw",
  "palette.navHistoryKw": "log past",
  "palette.navDevicesKw": "sync identity",
  "palette.navSettingsKw": "config relay name",
  "palette.themeLight": "Switch to the light theme",
  "palette.themeSystem": "Follow the system theme",
  "palette.themeDark": "Switch to the dark theme",
  "palette.themeKw": "theme dark light appearance",
  "palette.sendTo": (name: string) => `Send to ${name}`,
  "palette.verified": "verified",
  "palette.notVerified": "not verified",
  "palette.openCard": (name: string) => `Open ${name}'s card`,
  "palette.personKw": "fingerprint verify",
  "palette.label": "Search and run",
  "palette.placeholder": "Search commands and people…",
  "palette.noMatch": (q: string) => `Nothing matches “${q}”.`,

  // ---- store: failures and notices ---------------------------------------
  "store.unknownPeer": "unknown",
  // Our own id, wherever a row names its peer. Not a contact and not a stranger:
  // the other machines that are also us.
  "store.selfPeer": "Your devices",
  "store.loadTransfers": (e: string) =>
    `Couldn't read the transfers: ${e}`,
  "store.loadHistory": (e: string) =>
    `Couldn't read the history: ${e}`,
  "store.loadDeposits": (e: string) =>
    `Couldn't read the links: ${e}`,
  "store.loadConfig": (e: string) =>
    `Couldn't read the settings: ${e}`,
  "store.loadSync": (e: string) => `Couldn't read the device state: ${e}`,
  "store.errClearHistory": "Couldn't empty the history",
  "store.errRevokeLink": "Couldn't withdraw the link",
  "store.errSaveConfig": "Couldn't save the settings",
  "store.errSend": (to: string) => `Sending to ${to} failed`,
  "store.errDeposit": (to: string) => `The deposit for ${to} failed`,
  "store.errTicket": "Couldn't create the ticket",
  "store.errCode": "Couldn't create the code",
  "store.errLink": "Couldn't create the link",
  "store.errReceive": "Receiving failed",
  "store.errAccept": "Couldn't accept the file",
  "store.errReject": "Couldn't reject the file",
  "store.declined": (who: string) => who ? `Offer from ${who} declined.` : "Offer declined.",
  "bits.qrTooDense": "Too much data for a QR — use Copy.",
  "store.errPause": "Couldn't pause it",
  "store.errResume": "Couldn't resume it",
  "store.errDepositNow": "Couldn't leave it in the mailbox",
  "store.errCancel": "Couldn't cancel it",
  "store.errRemove": "Couldn't delete it",
  "store.errVerify": (name: string) => `Couldn't verify ${name}`,
  "store.errUnverify": (name: string) =>
    `Couldn't take the verification back from ${name}`,
  "store.errTrust": (who: string) => `Couldn't trust ${who}`,
  "store.errUntrust": (who: string) => `Couldn't take trust back from ${who}`,
  "store.errBlock": (who: string) => `Couldn't block ${who}`,
  "store.errUnblock": (who: string) => `Couldn't unblock ${who}`,
  "store.errAcceptName": (who: string) => `Couldn't approve ${who}'s name`,
  "store.errAddContact": (name: string) => `Couldn't save ${name}`,
  "store.errRemoveContact": (name: string) => `Couldn't remove ${name}`,
  "store.errRenameContact": (old: string) => `Couldn't rename ${old}`,
  "store.errSetMyName": "Couldn't set the name",
  "store.errRestartDaemon": "Couldn't restart the daemon",
  "store.errClearFinished": "Couldn't clear the finished ones",
  "store.syncFailed": "Sync didn't work",
  "store.syncOk": "Address book synced",
  "store.syncMerged": (n: number) =>
    n === 1
      ? "1 update from your other devices."
      : `${n} updates from your other devices.`,
  "store.syncNone": "No updates from your other devices.",
} as const satisfies Record<string, string | ((...a: never[]) => string)>;

/** The shape every other language has to fill. Deriving it from `en` is what
 *  makes a missing key a compile error rather than a hole in a shipped build. */
export type Dict = {
  [K in keyof typeof en]: (typeof en)[K] extends (...a: infer A) => string
    ? (...a: A) => string
    : string;
};
