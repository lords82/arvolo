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

  // ---- app frame ---------------------------------------------------------
  "title.transfers": "Transfers",
  "title.people": "People",
  "title.deposits": "Links and deposits",
  "title.history": "History",
  "title.devices": "Your devices",
  "title.settings": "Settings",

  "app.disconnected":
    "I can't reach the daemon. Transfers already running carry on, but this window can't see them.",
  "app.versionMismatch": (daemon: string, gui: string) =>
    `The running daemon is version ${daemon}, the app is ${gui}. Restart it to line them up.`,
  "app.versionUnknown": "older",
  "app.restart": "Restart",
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

  "crash.title": "Something broke in the interface",
  "crash.body":
    "Your transfers don't stop: they carry on in the background daemon. You can pick up where you left off.",

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
  "status.sharing": "Shared",

  "share.title": "Shared file",
  "share.stop": "Stop sharing",
  "share.copies": "copies taken",
  "share.now": "downloading now",
  "share.lastPickup": "last taken",
  "share.never": "never",
  "share.uploaded": "uploaded",
  "share.fromDownload": (when: string) =>
    `You downloaded this ${when}, and your computer is now making it available to others.`,
  "share.seedingSetting": "Change this",
  "share.countsNote":
    "Copies taken, not people: a ticket carries no identity, so the same person fetching twice counts twice.",
  "status.completed": "Completed",
  "status.deposited": "Deposited",
  "status.paused": "Paused",
  "status.incoming": "To confirm",
  "status.stalled": "Stalled",
  "status.failed": "Failed",
  "status.cancelling": "Cancelling…",
  "status.cancelled": "Cancelled",

  "method.p2p": "Direct",
  "method.cloud": "Mailbox",
  "method.link": "Link",
  "method.ticket": "Ticket",

  "meta.paused": "paused",
  "meta.sharing": "available — nobody downloading it",
  "meta.sharingPeers": (n: number) =>
    n === 1 ? "1 person downloading it" : `${n} people downloading it`,
  "meta.stalled": "resumes as soon as it can",
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
  "transfers.openFile": "Open file",
  "transfers.openFileFailed": "I can't open the file",
  "transfers.openFolder": "Open the folder",
  "transfers.openFolderFailed": "I can't open the folder",
  "transfers.revokeDeposit": "Withdraw the deposit",
  "transfers.cancel": "Cancel",
  "transfers.removeRow": "Take off the list",
  "transfers.verifiedIdentity": "Verified identity",
  "transfers.swarm": "Transfer spread across several peers",
  "transfers.peers": (n: number) => (n === 1 ? "1 peer" : `${n} peers`),
  "transfers.liveCode": "code live",
  "transfers.review": "Review",
  "transfers.shareDetails": "Sharing details",
  "transfers.reorder": (name: string) =>
    `Move ${name}: drag, or use the up and down arrows`,
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
  "transfers.firstRunBody":
    "Or pick a contact, generate a short code to read out, or create a link that opens in any browser. Everything is encrypted end to end: the relay only ever sees unreadable bytes.",
  "transfers.firstRunAction": "Send something",

  // ---- people ------------------------------------------------------------
  "people.presenceUnknownTitle": "No idea: the relay didn't answer",
  "people.presenceUnknownLabel": "Presence unknown",
  "people.presenceOnTitle": "Connected right now",
  "people.presenceOffTitle": "Not connected",
  "people.presenceOn": "Connected",
  "people.presenceOff": "Not connected",

  "people.menuDetails": "Details and fingerprint",
  "people.menuUnverify": "Take back the verification",
  "people.menuVerify": "Mark as verified…",
  "people.menuUntrust": "No longer trusted: ask me every time",
  "people.menuTrust": "Mark as trusted: download automatically",
  "people.menuUnblock": "Unblock",
  "people.menuBlock": "Block",
  "people.menuRemove": "Remove from the address book",
  "people.rowActions": (name: string) => `Actions for ${name}`,
  "people.goesBy": (name: string) => `goes by “${name}”`,
  "people.notVerified": "Not verified",
  "people.notVerifiedTitle": "The fingerprint has never been compared",
  "people.wantsToBeCalled": (name: string) => `Wants to be called “${name}”.`,
  "people.approve": "Approve",
  "people.send": "Send",
  "people.details": "Details",

  "people.confirmRemoveTitle": (name: string) => `Remove ${name}?`,
  "people.confirmRemoveBody":
    "They disappear from the address book along with their verified and trusted marks. Transfers already made stay in the history.",
  "people.confirmForceTitle": "Download automatically from an unverified key?",
  "people.confirmForceBody": (name: string) =>
    `Files from ${name} would be downloaded without asking you anything, but you have never compared their fingerprint in person. If somebody had got in the middle when you added them, you would be auto-downloading from that person.`,
  "people.confirmForceFooter":
    "The right way round is to compare the fingerprint and then mark them verified.",
  "people.confirmForceLabel": "Force it anyway",
  "people.confirmForceCancel": "I'll verify first",

  "people.addTitle": "Add by id",
  "people.addSubtitle": "The long way round: you need their public id in full.",
  "people.addNameLabel": "What you call them",
  "people.addNamePlaceholder": "e.g. Julia",
  "people.addIdLabel": "Public id",
  "people.addIdHint":
    "They find it with “arvolo me”, or on the Settings screen of their app.",
  "people.addTip":
    "Far simpler: Swap contacts. You read each other a short code and you both end up saved and already verified, without copying fifty characters.",
  "people.addSaved": (name: string) => `Saved ${name}`,
  "people.addSavedDetail":
    "They stay unverified until you compare the fingerprint.",

  "person.fingerprint": "Fingerprint",
  "person.fingerprintHint":
    "The same words have to appear on their screen. Compare them out loud or in person — not over a chat on the same channel you swapped ids on.",
  "person.publicId": "Public id",
  "person.verified": "Verified",
  "person.verifiedBody": "You have confirmed this fingerprint out of band.",
  "person.unverify": "Take back the verification",
  "person.notVerifiedYet": "Not verified yet",
  "person.notVerifiedBody":
    "Until you compare the fingerprint, the only thing you know is that somebody gave you that id.",
  "person.compared": (name: string) =>
    `I compared the fingerprint with ${name} outside this app.`,
  "person.markVerified": "Mark as verified",
  "person.rename": "Rename",
  "person.renameHint": "The name is yours: the key and the marks stay put.",

  "people.swap": "Swap contacts",
  "people.haveCode": "I have a code",
  "people.byId": "By id",
  "people.export": "Export",
  "people.import": "Import",
  "people.whoIsOnline": "Who's about",
  "people.whoIsOnlineTitle": "Ask the relay who is connected right now",
  "people.moreActions": "Other address-book actions",
  "people.prune": "Clean up orphaned names",
  "people.pruneNone": "Nothing to clean up",
  "people.pruneOne": "Removed 1 record",
  "people.pruneMany": (n: number) => `Removed ${n} records`,
  "people.pruneDetail":
    "They were names advertised by contacts you no longer have.",
  "people.filterLabel": "Address-book filter",
  "people.filterAll": "All",
  "people.filterVerified": "Verified",
  "people.filterTrusted": "Trusted",
  "people.filterBlocked": "Blocked",
  "people.filterBlockedN": (n: number) => `Blocked (${n})`,
  "people.searchPlaceholder": "Search by name or id…",
  "people.searchLabel": "Search the address book",
  "people.emptyNone": "Nobody in the address book",
  "people.emptyNoMatch": "No contact matches",
  "people.emptyNoneBody":
    "The quickest way to add somebody is to read them a short code: you save each other and you are verified straight away, with no ids to copy by hand.",
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
  "deposits.openFailed": "I can't open the link",
  "deposits.share": "Link",
  "deposits.shareTicket": "Ticket",
  "deposits.shareTicketTitle": "The ticket, again",
  "deposits.ticketDetail":
    "Paste it to the recipient: it opens in their Arvolo, or with `arvolo recv`. Only they can decrypt it — it is sealed to their identity.",
  "deposits.shareTitle": "The link, again",
  "deposits.publicLink": "Public link",
  "deposits.sealed": "Deposit",
  "deposits.revoke": "Withdraw",
  "deposits.sealedFor": (who: string, detail: string) =>
    `sealed for ${who} · ${detail}`,
  "deposits.confirmRevokeTitle": "Withdraw it?",
  "deposits.confirmRemoveTitle": "Remove the row?",
  "deposits.confirmRevokeLink":
    "The link stops working for everyone you gave it to, and anyone who has already downloaded it keeps their copy. The file stays on your disk.",
  "deposits.confirmRevokeSealed":
    "The file is taken off the relay and the offer withdrawn from the recipient's mailbox. If they haven't collected it yet, they no longer can.",
  "deposits.confirmRemoveBody":
    "There is nothing left on the relay to take away: only this row goes.",
  "deposits.intro":
    "What you have left on a relay and can still take back. The state is asked of the relay every time you open this screen — there is no other way to know it.",
  "deposits.createLink": "Create a link",
  "deposits.emptyTitle": "No live link or deposit",
  "deposits.emptyBody":
    "When you create a public link, or leave a file in somebody's mailbox, it shows up here — and this is where you take it back.",
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
    "Every linked device uses this one. To the rest of the world you are a single person, wherever you open Arvolo.",
  "devices.fingerprint": "Fingerprint",
  "devices.fingerprintHint":
    "It must be identical on all your devices. If one machine reads different words, that machine is not linked: it is another identity.",
  "devices.publicId": "Public id",
  "devices.pairTitle": "Link a device",
  "devices.pairBody":
    "Linking is done from both ends: on this machine you show a code, on the other you type it in. It is a delicate operation — what crosses over is your secret identity, not a mere invitation.",
  "devices.showCode": "Show a code",
  "devices.haveCode": "I have a code",
  "devices.pairWarnLead": "Never on a machine that isn't yours.",
  "devices.pairWarnRest":
    "Whoever types the code becomes you to every intent and purpose: same mailbox, same address book, same ability to open what is sent to you.",
  "devices.syncTitle": "Address book in step",
  "devices.syncHint":
    "Contacts travel between your devices inside an encrypted cell on your mailbox. The relay keeps bytes it cannot read.",
  "devices.syncOn": "On",
  "devices.syncOff": "Off",
  "devices.contactCount": (n: number) =>
    n === 1 ? "1 contact in the address book" : `${n} contacts in the address book`,
  "devices.lastSync": (when: string) => `last synced ${when}`,
  "devices.neverSynced": "not synced since the daemon started",
  "devices.lastError": (err: string) => `The last round failed: ${err}`,
  "devices.syncNow": "Sync now",
  "devices.autoTitle": "Sync on its own",
  "devices.autoDesc":
    "The daemon does a round every few minutes. Turn it off and the address book only lines up when you press the button above.",
  "devices.autoOn": "Automatic sync on",
  "devices.autoOff": "Automatic sync off",
  "devices.autoDetail": "It takes effect the next time the daemon starts.",

  // ---- settings ----------------------------------------------------------
  "settings.sourceEnv": "set by the ARVOLO_RELAY variable",
  "settings.sourceConfig": "saved in the settings",
  "settings.sourceBuiltin": "default, bundled with the app",
  "settings.sourceNone": "none",
  "settings.nameSaved": "Name updated",
  "settings.nameSavedDetail":
    "It travels inside every offer you send, from now on.",
  "settings.relaySaved": "Relay saved",
  "settings.relaySavedDetail":
    "The daemon will use it at its next start: restart it below to apply it right away.",
  "settings.whoYouAre": "Who you are",
  "settings.nameLabel": "The name you show",
  "settings.nameHint":
    "It travels inside every sealed offer you send. It is a label you pick yourself: whoever receives it sees it in quotes, because nothing guarantees it. The only thing that really identifies you is the fingerprint below.",
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
    "“System” follows the language your computer is set to, falling back to English when it is one Arvolo does not speak.",
  "settings.network": "Network",
  "settings.relayOn": "Relay live",
  "settings.relayOff": "No relay",
  "settings.relayLabel": "Relay",
  "settings.relayLocked":
    "Right now the ARVOLO_RELAY environment variable decides: what you type here would have no effect while it is set.",
  "settings.relayHint": (current: string, source: string) =>
    `In use now: ${current} — ${source}. An address with no scheme becomes https://; for a plaintext relay write the scheme out in full, like http://relay.local:6282.`,
  "settings.relayNone": "none",
  "settings.relayPlaceholder": "relay.example.com",
  "settings.relayNote":
    "The relay routes the codes, the mailbox and the links. It never sees your files in the clear: what it keeps is encrypted with keys it does not have.",
  "settings.files": "Files",
  "settings.downloadDirLabel": "Where received files land",
  "settings.downloadDirEnv": "Decided by the ARVOLO_DOWNLOAD_DIR variable.",
  "settings.downloadDirHint":
    "It applies to whatever you accept without picking a folder on the spot.",
  "settings.change": "Change",
  "settings.dirUpdated": "Folder updated",
  "settings.dirUpdatedDetail": "The daemon will use it at its next start.",
  "settings.cannotOpen": "I can't open it",
  "settings.seedTitle": "Carry on sharing what you have downloaded",
  "settings.seedDesc":
    "Leaving seeding on helps whoever is downloading the same file. You can switch it off if you would rather not stay in the swarm.",
  "settings.saved": "Setting saved",
  "settings.savedDetail": "It takes effect the next time the daemon starts.",
  "settings.advanced": "Advanced",
  "settings.configFileLabel": "Configuration file",
  "settings.configFileHint":
    "Everything that is not here — temp folder, NAT relay, log level — is set by hand in this file, which is commented line by line.",
  "settings.identityKeyLabel": "Identity key",
  "settings.identityKeyHint":
    "Your secret. Do not share it: whoever holds it becomes you. To use Arvolo on another machine of yours there is device linking, which hands it over encrypted.",
  "settings.versions": (daemon: string, gui: string) =>
    `Daemon ${daemon} · interface ${gui}`,
  "settings.restartDaemon": "Restart the daemon",
  "settings.confirmRestartTitle": "Restart the daemon?",
  "settings.confirmRestartBody":
    "Transfers under way stop: the resumable ones pick up where they were, the rest have to be done again from scratch. It is what applies a relay or a folder you have just changed.",
  "settings.restarting": "Daemon restarting",
  "settings.restartingDetail": "It comes back on its own in a few seconds.",
  "settings.refreshing": "Refreshing…",

  // ---- send sheet --------------------------------------------------------
  "send.modeContact": "To a contact",
  "send.modeCode": "Code",
  "send.modeLink": "Link",
  "send.modeTicket": "Ticket",
  "send.blurbContact":
    "Goes straight to somebody in your address book. If they are connected it passes directly from device to device; if they are not, it waits in their mailbox on the relay until they collect it.",
  "send.blurbCode":
    "A short code to read out or point a camera at. Whoever gets it pastes it into Arvolo — they need not be in your address book, but you both have to be connected right now.",
  "send.blurbLink":
    "An address that opens in any browser: whoever receives it needs neither Arvolo nor an account. The file is decrypted in the browser, the key travels in the URL fragment and never reaches the relay.",
  "send.blurbTicket":
    "An arvc… peer-to-peer ticket: it goes through neither the mailbox nor the Arvolo relay. Punching through NAT may need a connection relay, which only ever sees encrypted traffic.",
  "send.ttl1h": "1 hour",
  "send.ttl1d": "1 day",
  "send.ttl7d": "7 days",
  "send.ttl30d": "30 days",
  "send.pickerEmpty":
    "You have nobody in your address book yet. Add somebody from People — the quickest way is the code swap, which saves you both, already verified.",
  "send.pickerSearch": "Search a contact…",
  "send.pickerRecipient": "Recipient",
  "send.pickerNoMatch": (q: string) => `No contact matches “${q}”.`,
  "send.depositResult": (to: string) =>
    `Deposited for ${to}. The ticket below is your own copy: you only need it if you want to hand it over yourself — if ${to} never gets the offer, say.`,
  "send.onItsWay": (to: string) => `On its way to ${to}`,
  "send.onItsWayDetail":
    "If they are online it goes direct, otherwise it waits in their mailbox.",
  "send.codeKeepDetail":
    "The code stays good for several recipients until you cancel the send.",
  "send.codeOnceDetail":
    "The code is good for one recipient and then retires itself.",
  "send.linkDetail":
    "Anyone holding this address can download the file until it expires, runs out of allowed downloads, or you withdraw it from “Links and deposits”.",
  "send.ticketDetail":
    "Peer-to-peer ticket: good for as long as the daemon is running and the send has not been cancelled.",
  "send.countOne": "1 item",
  "send.countMany": (n: number) =>
    `${n} items · they will be packed into one archive`,
  "send.titleReady": "Ready",
  "send.title": "Send",
  "send.subtitleReady": "Hand over what you see below.",
  "send.subtitle": "Encrypted end to end, always.",
  "send.submitDeposit": "Deposit it",
  "send.submitSend": "Send",
  "send.submitCode": "Generate the code",
  "send.submitLink": "Create the link",
  "send.submitTicket": "Create the ticket",
  "send.linkKeyNote":
    "The link carries the key after the #: browsers do not send that part to the server, so the relay keeps only bytes it cannot read.",
  "send.filesLabel": "What you are sending",
  "send.filesHint": "You can also drag files and folders into the window.",
  "send.filesRemove": (name: string) => `Remove ${name}`,
  "send.pickFiles": "Files…",
  "send.pickFolder": "Folder…",
  "send.whoLabel": "Who it goes to",
  "send.modeLabel": "Way of sending",
  "send.noteLabel": "A couple of lines for the recipient (optional)",
  "send.noteHint":
    "It travels inside the sealed offer: the relay does not see it.",
  "send.notePlaceholder": "Here are the files we talked about.",
  "send.keepCodeTitle": "Good for several people",
  "send.keepCodeDesc":
    "By default the code is good for one recipient and then retires. Turn this on to leave it open until you cancel the send.",
  "send.keepCodeLabel": "Code good for several people",
  "send.depositTitle": "Leave it in the mailbox, don't wait",
  "send.depositDesc":
    "Deposit on the relay straight away even if they are connected: you close the window and forget about it. It unlocks expiry, number of collections and password.",
  "send.depositLabel": "Leave it in the mailbox",
  "send.expiresAfter": "Expires after",
  "send.depositTtlLabel": "Deposit expiry",
  "send.linkTtlLabel": "Link expiry",
  "send.maxPickupsLabel": "Collections allowed",
  "send.maxPickupsHint":
    "One, normally: the moment they download it, the relay deletes it.",
  "send.passwordLabel": "Password (optional)",
  "send.passwordHint":
    "It encrypts the deposit against the recipient too: without this password it does not open. The relay does not know it and cannot recover it — lose it and the file is lost.",
  "send.passwordPlaceholder": "none",
  "send.linkTooMany":
    "A link publishes one single item. Pick one, or put everything in a folder and select that.",
  "send.maxDownloadsLabel": "Downloads allowed",
  "send.maxDownloadsHint": "Leave it empty for no limit.",
  "send.maxDownloadsPlaceholder": "unlimited",
  "send.noRelay":
    "This way of sending needs a relay and none appears to be configured. Set one up under Settings.",
  "send.noArvoloRelay": "No Arvolo relay",

  // ---- receive sheet -----------------------------------------------------
  "receive.explainEmpty":
    "Paste a send code (like 4821-crater-mango) or an arvc… / arvm… ticket. To swap contacts with somebody, use People → I have a code instead.",
  "receive.explainCode":
    "Send code: I connect to whoever is showing it right now and download what they are sending.",
  "receive.explainChunk":
    "Peer-to-peer ticket: I download straight from the sender.",
  "receive.explainMailbox":
    "Mailbox ticket: I collect the file deposited on the relay.",
  "receive.explainUnknown":
    "I don't recognise this shape. I'll try it anyway — the daemon is fussier than I am — but check you copied it whole.",
  "receive.title": "Receive",
  "receive.subtitle": "Paste what you were given.",
  "receive.submit": "Receive",
  "receive.fieldLabel": "Code or ticket",
  "receive.passwordLabel": "Password (only if protected)",
  "receive.passwordHint":
    "Whoever sent it will have told you separately. Without it, a protected deposit does not open.",
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
  "incoming.later": "I'll decide later",
  "incoming.accept": "Accept and download",
  "incoming.notInBook": "Not in the address book",
  "incoming.claimedName": (name: string) =>
    `goes by “${name}” — a name they pick themselves, nothing guarantees it`,
  "incoming.keyFingerprint": "Fingerprint of the key",
  "incoming.senderId": "Sender's public id",
  "incoming.hintVerified":
    "You have already compared this fingerprint out of band: it is the same key you verified.",
  "incoming.hintKnown":
    "Compare it out loud with whoever is sending you the file. It is the only way to be sure it really is them — a name does not prove it.",
  "incoming.hintUnknown":
    "This is not a fingerprint: it is the raw id of somebody who is not in your address book. Save them below and you will see the words to compare out loud with them.",
  "incoming.attachedNote": "Message attached",
  "incoming.passwordLabel": "Password",
  "incoming.passwordHint":
    "This file is protected: without the password it does not open. Whoever sent it will have told you separately — it does not travel with the file, and the relay does not know it.",
  "incoming.ifYouKnowThem": "If you know them",
  "incoming.saveAsPlaceholder": "Save to the address book as…",
  "incoming.saveAsLabel": "Name to give the contact",
  "incoming.savedAs": (name: string) => `Saved as ${name}`,
  "incoming.savedAsDetail":
    "They stay unverified: confirm the fingerprint out loud and then mark them under People.",
  "incoming.saveNote":
    "Saving them does not verify them. They become verified only when you compare the fingerprint in person or out loud.",
  "incoming.blockAndReject": "Block and reject",
  "incoming.blocked": "Blocked",
  "incoming.blockedDetail":
    "Their offers will be dropped on arrival, with no notice to you.",

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
  "pair.restartingDetail":
    "It is coming back up with the shared identity: a few seconds and everything is back.",
  "pair.restartAndClose": "Restart and close",
  "pair.link": "Link",
  "pair.done": "Done",
  "pair.needsRestart":
    "The daemon is still running with the previous identity. It has to be restarted for the change to take effect — the button below does it.",
  "pair.failed": "That didn't work",
  "pair.deviceWarnLead": "This shares your secret identity.",
  "pair.deviceWarnRest":
    "Whoever types the code becomes you: same public id, same mailbox, same address book. Use it only on a machine of your own. The code is good for one device and expires the moment it is used.",
  "pair.captionDevice":
    "On the other device: Your devices → I have a code.",
  "pair.captionContact":
    "Read it out to them. They open People → I have a code and type it in.",
  "pair.waitingOther": "Waiting for the other side… you can close to cancel.",
  "pair.preparingCode": "Preparing the code…",
  "pair.contactNote":
    "Only public ids are exchanged. Your secret identity and your address book do not leave this machine.",
  "pair.joinWarnLead": "Careful: this cannot be undone.",
  "pair.joinWarnRest":
    "This device's current identity is replaced by the shared one. Anything still sealed to the old identity stops being openable here.",
  "pair.codeLabel": "Code",
  "pair.codeHint":
    "The one shown on the other machine, something like 4821-crater-mango.",
  "pair.nameLabel": "What you call them (optional)",
  "pair.nameHint":
    "Leave it empty and I'll save them under a name derived from their fingerprint; you can rename them whenever you like.",
  "pair.understood":
    "I understand: this device loses its current identity.",
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
  "palette.pairContact": "Swap contacts with somebody",
  "palette.pairContactHint": "you save each other, already verified",
  "palette.pairContactKw": "pairing pair add person verify",
  "palette.pairDevice": "Link another device of yours",
  "palette.pairDeviceKw": "multidevice identity sync",
  "palette.sync": "Sync the address book now",
  "palette.syncKw": "contacts devices",
  "palette.resumeAll": "Resume every transfer",
  "palette.pauseAll": "Pause every transfer",
  "palette.pauseAllKw": "pause all stop hold resume",
  "palette.clearFinished": "Clear the finished transfers",
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
  "palette.placeholder": "Search a command or a person…",
  "palette.noMatch": (q: string) => `Nothing matches “${q}”.`,

  // ---- store: failures and notices ---------------------------------------
  "store.unknownPeer": "unknown",
  "store.loadTransfers": (e: string) =>
    `I can't read the transfers from the daemon: ${e}`,
  "store.loadHistory": (e: string) =>
    `I can't read the history from the daemon: ${e}`,
  "store.loadDeposits": (e: string) =>
    `I can't read the links from the daemon: ${e}`,
  "store.loadConfig": (e: string) =>
    `I can't read the settings from the daemon: ${e}`,
  "store.loadSync": (e: string) => `I can't read the device state: ${e}`,
  "store.errClearHistory": "Couldn't empty the history",
  "store.errRevokeLink": "Couldn't withdraw the link",
  "store.errSaveConfig": "Couldn't save the settings",
  "store.errPruneNames": "Couldn't clean up the names",
  "store.errSend": (to: string) => `Sending to ${to} failed`,
  "store.errDeposit": (to: string) => `The deposit for ${to} failed`,
  "store.errTicket": "Couldn't create the ticket",
  "store.errCode": "Couldn't create the code",
  "store.errLink": "Couldn't create the link",
  "store.errReceive": "Receiving failed",
  "store.errAccept": "Couldn't accept the file",
  "store.errReject": "Couldn't reject the file",
  "store.errPause": "Couldn't pause it",
  "store.errResume": "Couldn't resume it",
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
