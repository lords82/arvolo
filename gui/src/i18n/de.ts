// Deutsch.
//
// Zwei Konventionen, die hier durchgehend gelten: das „Sie“, weil die App
// jemanden anspricht, den sie nicht kennt; und deutsche Anführungszeichen
// („…“) statt der englischen, weil zitierte Namen an genau den Stellen
// auftauchen, an denen es darauf ankommt, dass sie als Zitat lesbar sind.

import type { Dict } from "./en";

export const de: Dict = {
  "locale.tag": "de",
  "locale.name": "Deutsch",

  "common.cancel": "Abbrechen",
  "common.save": "Speichern",
  "common.done": "Fertig",
  "common.close": "Schließen",
  "common.confirm": "Bestätigen",
  "common.retry": "Erneut versuchen",
  "common.open": "Öffnen",
  "common.remove": "Entfernen",
  "common.refresh": "Aktualisieren",
  "common.copy": "Kopieren",
  "common.copied": "Kopiert",
  "common.copyFailed": "Kopieren fehlgeschlagen",
  "common.loading": "Wird geladen…",
  "common.to": "an",
  "common.from": "von",

  "title.transfers": "Übertragungen",
  "title.people": "Personen",
  "title.deposits": "Links und Ablagen",
  "title.history": "Verlauf",
  "title.devices": "Ihre Geräte",
  "title.settings": "Einstellungen",

  "app.disconnected":
    "Ich erreiche den Daemon nicht. Laufende Übertragungen gehen weiter, aber dieses Fenster sieht sie nicht.",
  "app.versionMismatch": (daemon, gui) =>
    `Der laufende Daemon hat Version ${daemon}, die App ${gui}. Starten Sie ihn neu, damit beide zusammenpassen.`,
  "app.versionUnknown": "älter",
  "app.restart": "Neu starten",
  "app.offerWaiting": "Jemand möchte Ihnen eine Datei schicken.",
  "app.offersWaiting": (n) => `${n} Dateien warten auf Ihre Bestätigung.`,
  "app.seeOffers": "Ansehen",
  "app.searchPlaceholder": "Nach Name oder Person filtern…",
  "app.searchLabel": "Übertragungen filtern",
  "app.clearFinished": (n) => `Aufräumen (${n})`,
  "app.palette": (mod) => `Suchen und ausführen (${mod}K)`,
  "app.send": "Senden",
  "app.sendShortcut": (mod) => `Senden (${mod}N)`,
  "app.dropTitle": "Zum Senden hier ablegen",
  "app.dropHint": "Danach wählen Sie, an wen: Kontakt, Code oder Link.",
  "app.actionFailed": "Das hat nicht geklappt",

  "crash.title": "In der Oberfläche ist etwas kaputtgegangen",
  "crash.body":
    "Ihre Übertragungen halten nicht an: Sie laufen im Hintergrund im Daemon weiter. Sie können dort weitermachen, wo Sie waren.",

  "rail.nav": "Hauptnavigation",
  "rail.meTitle": "Ihre Identität und die Einstellungen",
  "rail.meFallback": "Ich",
  "rail.noIdentity": "Identität noch nicht gelesen",
  "rail.daemonUp": "Daemon verbunden",
  "rail.daemonDown": "Daemon nicht erreichbar",
  "rail.send": "Senden…",
  "rail.receive": "Empfangen…",
  "rail.sections": "Bereiche",
  "rail.palette": "Suchen und ausführen",

  "status.active": "Läuft",
  "status.sharing": "Geteilt",

  "share.title": "Geteilte Datei",
  "share.stop": "Teilen beenden",
  "share.copies": "abgeholte Kopien",
  "share.now": "laden gerade",
  "share.lastPickup": "zuletzt abgeholt",
  "share.never": "nie",
  "share.uploaded": "hochgeladen",
  "share.fromDownload": (when: string) =>
    `Sie haben das ${when} heruntergeladen, und Ihr Rechner stellt es jetzt anderen zur Verfügung.`,
  "share.seedingSetting": "Einstellung ändern",
  "share.countsNote":
    "Kopien, keine Personen: Ein Ticket trägt keine Identität, dieselbe Person z\u00e4hlt beim zweiten Abholen erneut.",
  "status.completed": "Abgeschlossen",
  "status.deposited": "Abgelegt",
  "status.paused": "Pausiert",
  "status.incoming": "Zu bestätigen",
  "status.stalled": "Wartet",
  "status.failed": "Fehlgeschlagen",
  "status.cancelling": "Wird abgebrochen…",
  "status.cancelled": "Abgebrochen",

  "method.p2p": "Direkt",
  "method.cloud": "Postfach",
  "method.link": "Link",
  "method.ticket": "Ticket",

  "meta.paused": "pausiert",
  "meta.sharing": "verfügbar — niemand lädt es gerade",
  "meta.sharingPeers": (n) =>
    n === 1 ? "1 Person lädt es" : `${n} Personen laden es`,
  "meta.stalled": "läuft weiter, sobald es geht",
  "meta.incoming": "für Details öffnen",
  "meta.deposited": "wartet darauf, abgeholt zu werden",
  "meta.failed": "Übertragung fehlgeschlagen",

  "eta.seconds": (n) => `${n} s`,
  "eta.minutes": (n) => `${n} Min.`,
  "eta.hours": (n) => `${n} Std.`,
  "until.seconds": (n) => (n === 1 ? "1 Sekunde" : `${n} Sekunden`),
  "until.minutes": (n) => (n === 1 ? "1 Minute" : `${n} Minuten`),
  "until.hours": (n) => (n === 1 ? "1 Stunde" : `${n} Stunden`),
  "until.days": (n) => (n === 1 ? "1 Tag" : `${n} Tagen`),
  "ago.moments": "vor wenigen Sekunden",
  "ago.minutes": (n) => (n === 1 ? "vor 1 Minute" : `vor ${n} Minuten`),
  "ago.hours": (n) => (n === 1 ? "vor 1 Stunde" : `vor ${n} Stunden`),
  "ago.days": (n) => (n === 1 ? "vor 1 Tag" : `vor ${n} Tagen`),

  "section.pending": "Zu bestätigen",
  "section.active": "Laufend und pausiert",
  "section.today": "Heute",
  "section.earlier": "Früher",

  "transfers.pause": "Pausieren",
  "transfers.resume": "Fortsetzen",
  "transfers.openFile": "Datei öffnen",
  "transfers.openFileFailed": "Ich kann die Datei nicht öffnen",
  "transfers.openFolder": "Ordner öffnen",
  "transfers.openFolderFailed": "Ich kann den Ordner nicht öffnen",
  "transfers.revokeDeposit": "Ablage zurückziehen",
  "transfers.cancel": "Abbrechen",
  "transfers.removeRow": "Von der Liste nehmen",
  "transfers.verifiedIdentity": "Verifizierte Identität",
  "transfers.swarm": "Übertragung auf mehrere Peers verteilt",
  "transfers.peers": (n) => (n === 1 ? "1 Peer" : `${n} Peers`),
  "transfers.liveCode": "Code aktiv",
  "transfers.review": "Ansehen",
  "transfers.shareDetails": "Details zum Teilen",
  "transfers.reorder": (name) =>
    `${name} verschieben: ziehen oder die Pfeiltasten hoch/runter benutzen`,
  "transfers.rowActions": (name) => `Aktionen für ${name}`,
  "transfers.progressOf": (name) => `Fortschritt von ${name}`,
  "transfers.confirmRevokeTitle": "Ablage zurückziehen?",
  "transfers.confirmCancelTitle": "Abbrechen?",
  "transfers.confirmRevokeBody": (peer) =>
    `Die Datei wird vom Relay entfernt und das Angebot aus dem Postfach von ${peer} zurückgezogen. Herunterladen ist dann nicht mehr möglich.`,
  "transfers.confirmRevokePeer": "der Empfängerseite",
  "transfers.confirmCancelBody": (name) =>
    `„${name}“ endet hier. Was schon übertragen wurde, wird verworfen: beim nächsten Versuch fängt es von vorn an.`,
  "transfers.confirmRevokeLabel": "Zurückziehen",
  "transfers.confirmCancelLabel": "Übertragung abbrechen",
  "transfers.keepGoing": "Doch nicht",
  "transfers.outgoing": "Ausgehend",
  "transfers.incoming": "Eingehend",
  "transfers.emptyOutTitle": "Nichts ausgehend",
  "transfers.emptyInTitle": "Nichts eingehend",
  "transfers.emptyOutBody":
    "Ziehen Sie eine Datei ins Fenster, oder nutzen Sie Senden.",
  "transfers.emptyInBody":
    "Hier erscheinen die Dateien, die Ihnen jemand schickt.",
  "transfers.emptyOutAction": "Etwas senden",
  "transfers.emptyInAction": "Code einfügen",
  "transfers.firstRunTitle": "Ziehen Sie die Dateien zum Senden hierher",
  "transfers.firstRunBody":
    "Oder wählen Sie einen Kontakt, erzeugen Sie einen kurzen Code zum Vorlesen, oder erstellen Sie einen Link, der sich in jedem Browser öffnet. Alles ist Ende-zu-Ende verschlüsselt: das Relay sieht nur unlesbare Bytes.",
  "transfers.firstRunAction": "Etwas senden",

  "people.presenceUnknownTitle": "Weiß ich nicht: das Relay hat nicht geantwortet",
  "people.presenceUnknownLabel": "Anwesenheit unbekannt",
  "people.presenceOnTitle": "Gerade verbunden",
  "people.presenceOffTitle": "Nicht verbunden",
  "people.presenceOn": "Verbunden",
  "people.presenceOff": "Nicht verbunden",

  "people.menuDetails": "Details und Fingerabdruck",
  "people.menuUnverify": "Verifizierung zurücknehmen",
  "people.menuVerify": "Als verifiziert markieren…",
  "people.menuUntrust": "Nicht mehr vertrauenswürdig: jedes Mal nachfragen",
  "people.menuTrust": "Als vertrauenswürdig markieren: automatisch laden",
  "people.menuUnblock": "Entsperren",
  "people.menuBlock": "Sperren",
  "people.menuRemove": "Aus dem Adressbuch entfernen",
  "people.rowActions": (name) => `Aktionen für ${name}`,
  "people.goesBy": (name) => `nennt sich „${name}“`,
  "people.notVerified": "Nicht verifiziert",
  "people.notVerifiedTitle": "Der Fingerabdruck wurde nie verglichen",
  "people.wantsToBeCalled": (name) => `Möchte „${name}“ genannt werden.`,
  "people.approve": "Übernehmen",
  "people.send": "Senden",
  "people.details": "Details",

  "people.confirmRemoveTitle": (name) => `${name} entfernen?`,
  "people.confirmRemoveBody":
    "Verschwindet mitsamt den Markierungen für Verifizierung und Vertrauen aus dem Adressbuch. Bereits erfolgte Übertragungen bleiben im Verlauf.",
  "people.confirmForceTitle":
    "Von einem nicht verifizierten Schlüssel automatisch laden?",
  "people.confirmForceBody": (name) =>
    `Dateien von ${name} würden ohne Rückfrage geladen, obwohl Sie den Fingerabdruck nie persönlich verglichen haben. Hätte sich beim Hinzufügen jemand dazwischengeschaltet, würden Sie automatisch von dieser Person laden.`,
  "people.confirmForceFooter":
    "Der richtige Weg ist, den Fingerabdruck zu vergleichen und den Kontakt dann als verifiziert zu markieren.",
  "people.confirmForceLabel": "Trotzdem erzwingen",
  "people.confirmForceCancel": "Ich verifiziere erst",

  "people.addTitle": "Über die ID hinzufügen",
  "people.addSubtitle":
    "Der lange Weg: Sie brauchen die vollständige öffentliche ID.",
  "people.addNameLabel": "Wie Sie die Person nennen",
  "people.addNamePlaceholder": "z. B. Julia",
  "people.addIdLabel": "Öffentliche ID",
  "people.addIdHint":
    "Sie findet sie mit „arvolo me“, oder im Einstellungen-Fenster ihrer App.",
  "people.addTip":
    "Viel einfacher: Kontakte tauschen. Sie lesen einander einen kurzen Code vor und sind beide gespeichert und schon verifiziert, ohne fünfzig Zeichen abzutippen.",
  "people.addSaved": (name) => `${name} gespeichert`,
  "people.addSavedDetail":
    "Bleibt unverifiziert, bis Sie den Fingerabdruck vergleichen.",

  "person.fingerprint": "Fingerabdruck",
  "person.fingerprintHint":
    "Dieselben Wörter müssen auf dem anderen Bildschirm stehen. Vergleichen Sie sie mündlich oder persönlich — nicht per Chat über denselben Kanal, über den Sie die IDs getauscht haben.",
  "person.publicId": "Öffentliche ID",
  "person.verified": "Verifiziert",
  "person.verifiedBody":
    "Sie haben diesen Fingerabdruck außerhalb des Kanals bestätigt.",
  "person.unverify": "Verifizierung zurücknehmen",
  "person.notVerifiedYet": "Noch nicht verifiziert",
  "person.notVerifiedBody":
    "Solange Sie den Fingerabdruck nicht vergleichen, wissen Sie nur, dass Ihnen jemand diese ID gegeben hat.",
  "person.compared": (name) =>
    `Ich habe den Fingerabdruck mit ${name} außerhalb dieser App verglichen.`,
  "person.markVerified": "Als verifiziert markieren",
  "person.rename": "Umbenennen",
  "person.renameHint":
    "Der Name gehört Ihnen: Schlüssel und Markierungen bleiben.",

  "people.swap": "Kontakte tauschen",
  "people.haveCode": "Ich habe einen Code",
  "people.byId": "Über die ID",
  "people.export": "Exportieren",
  "people.import": "Importieren",
  "people.whoIsOnline": "Wer ist da",
  "people.whoIsOnlineTitle": "Das Relay fragen, wer gerade verbunden ist",
  "people.moreActions": "Weitere Adressbuch-Aktionen",
  "people.prune": "Verwaiste Namen aufräumen",
  "people.pruneNone": "Nichts aufzuräumen",
  "people.pruneOne": "1 Eintrag entfernt",
  "people.pruneMany": (n) => `${n} Einträge entfernt`,
  "people.pruneDetail":
    "Das waren Namen von Kontakten, die Sie nicht mehr haben.",
  "people.filterLabel": "Adressbuch-Filter",
  "people.filterAll": "Alle",
  "people.filterVerified": "Verifiziert",
  "people.filterTrusted": "Vertrauenswürdig",
  "people.filterBlocked": "Gesperrt",
  "people.filterBlockedN": (n) => `Gesperrt (${n})`,
  "people.searchPlaceholder": "Nach Name oder ID suchen…",
  "people.searchLabel": "Im Adressbuch suchen",
  "people.emptyNone": "Niemand im Adressbuch",
  "people.emptyNoMatch": "Kein Kontakt passt",
  "people.emptyNoneBody":
    "Am schnellsten fügen Sie jemanden hinzu, indem Sie ihm einen kurzen Code vorlesen: Sie speichern einander und sind sofort verifiziert, ganz ohne IDs abzutippen.",
  "people.emptyNoMatchBody": "Versuchen Sie einen anderen Filter oder Suchtext.",
  "people.exportFilename": "arvolo-kontakte.json",
  "people.exportedOne": "1 Kontakt exportiert",
  "people.exportedMany": (n) => `${n} Kontakte exportiert`,
  "people.exportDetail":
    "Die Datei enthält nur öffentliche IDs: keine Geheimnisse.",
  "people.exportFailed": "Export fehlgeschlagen",
  "people.importedOne": "1 Kontakt importiert",
  "people.importedMany": (n) => `${n} Kontakte importiert`,
  "people.importDetail": (skipped) =>
    `${skipped ? `${skipped} übersprungen. ` : ""}Alle unverifiziert: die Markierungen werden nicht importiert, weil Sie diese Fingerabdrücke nicht selbst geprüft haben.`,
  "people.importFailed": "Import fehlgeschlagen",
  "people.importNotAList": "die Datei ist keine Liste",

  "trust.blocked": "Gesperrt",
  "trust.blockedTitle": "Angebote dieser Person werden bei Ankunft verworfen",
  "trust.verified": "Verifiziert",
  "trust.verifiedTitle": "Fingerabdruck außerhalb des Kanals bestätigt",
  "trust.trusted": "Vertrauenswürdig",
  "trust.trustedTitle": "Dateien werden ohne Rückfrage geladen",

  "deposit.expired": "Abgelaufen",
  "deposit.expiredDetail": "die Frist ist verstrichen",
  "deposit.taken": "Abgeholt",
  "deposit.takenDetail": "die empfangende Person hat es heruntergeladen",
  "deposit.offerPending": "noch nicht bei ihnen angekommen",
  "deposit.offerArrived": "auf ihrem Gerät angekommen, noch nicht abgeholt",
  "deposit.gone": "Nicht mehr verfügbar",
  "deposit.goneLink": "bis zum Limit geladen, oder bereits zurückgezogen",
  "deposit.goneSealed": "abgeholt, oder bereits zurückgezogen",
  "deposit.expiresIn": (until) => `läuft in ${until} ab`,
  "deposit.expiredJustNow": "Frist gerade verstrichen",
  "deposit.unknown": "Zustand unbekannt",
  "deposit.unknownDetail": (when) => `Relay nicht erreichbar · ${when}`,
  "deposit.downloads": (n, cap) => `${n}${cap} Downloads`,
  "deposit.noLimit": "kein Limit",
  "deposit.max": (label) => `max. ${label}`,
  "deposit.active": "Aktiv",

  "deposits.openInBrowser": "Im Browser öffnen",
  "deposits.openFailed": "Ich kann den Link nicht öffnen",
  "deposits.share": "Link",
  "deposits.shareTicket": "Ticket",
  "deposits.shareTicketTitle": "Das Ticket, noch einmal",
  "deposits.ticketDetail":
    "Fügen Sie es der Empfängerseite ein: es öffnet sich in deren Arvolo, oder mit `arvolo recv`. Entschlüsseln kann es nur sie — es ist auf ihre Identität versiegelt.",
  "deposits.shareTitle": "Der Link, noch einmal",
  "deposits.publicLink": "Öffentlicher Link",
  "deposits.sealed": "Ablage",
  "deposits.revoke": "Zurückziehen",
  "deposits.sealedFor": (who, detail) => `versiegelt für ${who} · ${detail}`,
  "deposits.confirmRevokeTitle": "Zurückziehen?",
  "deposits.confirmRemoveTitle": "Zeile entfernen?",
  "deposits.confirmRevokeLink":
    "Der Link hört für alle auf zu funktionieren, denen Sie ihn gegeben haben; wer schon geladen hat, behält seine Kopie. Die Datei bleibt auf Ihrer Platte.",
  "deposits.confirmRevokeSealed":
    "Die Datei wird vom Relay genommen und das Angebot aus dem Postfach der Empfängerseite zurückgezogen. Wer sie noch nicht abgeholt hat, kann es dann nicht mehr.",
  "deposits.confirmRemoveBody":
    "Auf dem Relay ist nichts mehr wegzunehmen: es verschwindet nur diese Zeile.",
  "deposits.intro":
    "Was Sie auf einem Relay liegen haben und noch zurückholen können. Der Zustand wird bei jedem Öffnen dieses Fensters beim Relay erfragt — anders lässt er sich nicht wissen.",
  "deposits.createLink": "Link erstellen",
  "deposits.emptyTitle": "Kein aktiver Link, keine Ablage",
  "deposits.emptyBody":
    "Wenn Sie einen öffentlichen Link erstellen oder eine Datei in jemandes Postfach legen, taucht sie hier auf — und von hier holen Sie sie zurück.",
  "deposits.sectionLinks": "Öffentliche Links",
  "deposits.sectionSealed": "Versiegelte Ablagen",

  "history.today": "Heute",
  "history.yesterday": "Gestern",
  "history.completed": "Abgeschlossen",
  "history.cancelled": "Abgebrochen",
  "history.deposited": "Abgelegt",
  "history.failed": "Fehlgeschlagen",
  "history.unknownOutcome": "Ausgang unbekannt",
  "history.filterLabel": "Verlaufsfilter",
  "history.filterAll": "Alles",
  "history.filterSent": "Gesendet",
  "history.filterReceived": "Empfangen",
  "history.searchPlaceholder": "Suchen…",
  "history.searchLabel": "Im Verlauf suchen",
  "history.clear": "Leeren",
  "history.emptyNoMatch": "Keine Treffer",
  "history.emptyNothing": "Noch nichts",
  "history.emptyNoMatchBody":
    "Versuchen Sie einen anderen Filter oder Suchtext.",
  "history.emptyNothingBody":
    "Hier landet jede abgeschlossene Übertragung: was, mit wem, und wie es ausging.",
  "history.confirmClearTitle": "Verlauf leeren?",
  "history.confirmClearBody":
    "Das Protokoll wird vollständig vergessen und lässt sich nicht wiederherstellen. Bereits empfangene Dateien bleiben, wo sie sind; das hier löscht nur die Liste.",

  "devices.identityTitle": "Ihre gemeinsame Identität",
  "devices.identityHint":
    "Jedes verbundene Gerät benutzt diese eine. Für den Rest der Welt sind Sie eine einzige Person, wo immer Sie Arvolo öffnen.",
  "devices.fingerprint": "Fingerabdruck",
  "devices.fingerprintHint":
    "Er muss auf allen Ihren Geräten gleich sein. Zeigt ein Rechner andere Wörter, ist er nicht verbunden: das ist eine andere Identität.",
  "devices.publicId": "Öffentliche ID",
  "devices.pairTitle": "Ein Gerät verbinden",
  "devices.pairBody":
    "Das Verbinden geschieht von beiden Seiten: auf diesem Rechner zeigen Sie einen Code, auf dem anderen geben Sie ihn ein. Das ist heikel — was übergeht, ist Ihre geheime Identität, keine bloße Einladung.",
  "devices.showCode": "Code anzeigen",
  "devices.haveCode": "Ich habe einen Code",
  "devices.pairWarnLead": "Niemals auf einem Rechner, der nicht Ihrer ist.",
  "devices.pairWarnRest":
    "Wer den Code eingibt, wird in jeder Hinsicht zu Ihnen: gleiches Postfach, gleiches Adressbuch, dieselbe Fähigkeit zu öffnen, was Ihnen geschickt wird.",
  "devices.syncTitle": "Adressbuch im Gleichstand",
  "devices.syncHint":
    "Kontakte wandern zwischen Ihren Geräten in einer verschlüsselten Zelle in Ihrem Postfach. Das Relay verwahrt Bytes, die es nicht lesen kann.",
  "devices.syncOn": "Aktiv",
  "devices.syncOff": "Deaktiviert",
  "devices.contactCount": (n) =>
    n === 1 ? "1 Kontakt im Adressbuch" : `${n} Kontakte im Adressbuch`,
  "devices.lastSync": (when) => `zuletzt abgeglichen ${when}`,
  "devices.neverSynced": "seit dem Start des Daemons noch nicht abgeglichen",
  "devices.lastError": (err) => `Die letzte Runde ist fehlgeschlagen: ${err}`,
  "devices.syncNow": "Jetzt abgleichen",
  "devices.autoTitle": "Von allein abgleichen",
  "devices.autoDesc":
    "Der Daemon dreht alle paar Minuten eine Runde. Wenn Sie das abschalten, gleicht sich das Adressbuch nur ab, wenn Sie oben auf den Knopf drücken.",
  "devices.autoOn": "Automatischer Abgleich aktiv",
  "devices.autoOff": "Automatischer Abgleich aus",
  "devices.autoDetail": "Wirkt beim nächsten Start des Daemons.",

  "settings.sourceEnv": "durch die Variable ARVOLO_RELAY vorgegeben",
  "settings.sourceConfig": "in den Einstellungen gespeichert",
  "settings.sourceBuiltin": "Standard, mit der App ausgeliefert",
  "settings.sourceNone": "keiner",
  "settings.nameSaved": "Name aktualisiert",
  "settings.nameSavedDetail":
    "Er reist ab sofort in jedem Angebot mit, das Sie senden.",
  "settings.relaySaved": "Relay gespeichert",
  "settings.relaySavedDetail":
    "Der Daemon benutzt es beim nächsten Start: starten Sie ihn unten neu, um es sofort anzuwenden.",
  "settings.whoYouAre": "Wer Sie sind",
  "settings.nameLabel": "Angezeigter Name",
  "settings.nameHint":
    "Er reist in jedem versiegelten Angebot mit, das Sie senden. Es ist ein Etikett, das Sie selbst wählen: die Gegenseite sieht es in Anführungszeichen, weil nichts dafür bürgt. Was Sie wirklich identifiziert, ist der Fingerabdruck darunter.",
  "settings.namePlaceholder": "keiner",
  "settings.fingerprintLabel": "Ihr Fingerabdruck",
  "settings.fingerprintHint":
    "Die Wörter, die andere vergleichen, um sicher zu sein, dass Sie es sind. Lesen Sie sie vor, wenn jemand Sie hinzufügt.",
  "settings.publicIdLabel": "Ihre öffentliche ID",
  "settings.appearance": "Darstellung",
  "settings.theme": "Erscheinungsbild",
  "settings.themeSystem": "System",
  "settings.themeLight": "Hell",
  "settings.themeDark": "Dunkel",
  "settings.language": "Sprache",
  "settings.languageAuto": "System",
  "settings.languageHint":
    "„System“ folgt der Sprache Ihres Rechners und fällt auf Englisch zurück, wenn es eine ist, die Arvolo nicht spricht.",
  "settings.startup": "Start",
  "settings.autostartTitle": "Beim Anmelden starten",
  "settings.autostartDesc":
    "Beim Anmelden startet Arvolo verborgen im Infobereich, empfangsbereit ohne offenes Fenster. Unter Linux zeigt der Eintrag auf den aktuellen Ort des AppImage: Wird die Datei verschoben, den Schalter einmal aus- und wieder einschalten.",
  "settings.autostartFailed": "Automatischer Start ließ sich nicht ändern",
  "settings.network": "Netzwerk",
  "settings.relayOn": "Relay aktiv",
  "settings.relayOff": "Kein Relay",
  "settings.relayLabel": "Relay",
  "settings.relayLocked":
    "Im Moment entscheidet die Umgebungsvariable ARVOLO_RELAY: was Sie hier schreiben, hätte keine Wirkung, solange sie gesetzt ist.",
  "settings.relayHint": (current, source) =>
    `Gerade in Benutzung: ${current} — ${source}. Eine Adresse ohne Schema wird zu https://; für ein unverschlüsseltes Relay schreiben Sie das Schema aus, etwa http://relay.local:6282.`,
  "settings.relayNone": "keines",
  "settings.relayPlaceholder": "relay.beispiel.de",
  "settings.relayNote":
    "Das Relay vermittelt die Codes, das Postfach und die Links. Es sieht Ihre Dateien nie im Klartext: was es aufbewahrt, ist mit Schlüsseln verschlüsselt, die es nicht hat.",
  "settings.files": "Dateien",
  "settings.downloadDirLabel": "Wohin empfangene Dateien kommen",
  "settings.downloadDirEnv":
    "Wird von der Variable ARVOLO_DOWNLOAD_DIR bestimmt.",
  "settings.downloadDirHint":
    "Gilt für alles, was Sie annehmen, ohne dabei einen Ordner zu wählen.",
  "settings.change": "Ändern",
  "settings.dirUpdated": "Ordner aktualisiert",
  "settings.dirUpdatedDetail":
    "Der Daemon benutzt ihn beim nächsten Start.",
  "settings.cannotOpen": "Ich kann das nicht öffnen",
  "settings.seedTitle": "Weiterhin teilen, was Sie geladen haben",
  "settings.seedDesc":
    "Aktives Seeding hilft allen, die dieselbe Datei laden. Sie können es abschalten, wenn Sie lieber nicht im Schwarm bleiben.",
  "settings.saved": "Einstellung gespeichert",
  "settings.savedDetail": "Wirkt beim nächsten Start des Daemons.",
  "settings.advanced": "Erweitert",
  "settings.configFileLabel": "Konfigurationsdatei",
  "settings.configFileHint":
    "Alles, was hier nicht auftaucht — temporärer Ordner, NAT-Relay, Log-Stufe — stellen Sie von Hand in dieser Datei ein, die Zeile für Zeile kommentiert ist.",
  "settings.identityKeyLabel": "Identitätsschlüssel",
  "settings.identityKeyHint":
    "Ihr Geheimnis. Geben Sie es nicht weiter: wer es hat, ist Sie. Um Arvolo auf einem weiteren eigenen Rechner zu nutzen, gibt es die Geräteverbindung, die es verschlüsselt überträgt.",
  "settings.versions": (daemon, gui) =>
    `Daemon ${daemon} · Oberfläche ${gui}`,
  "settings.restartDaemon": "Daemon neu starten",
  "settings.confirmRestartTitle": "Daemon neu starten?",
  "settings.confirmRestartBody":
    "Laufende Übertragungen halten an: die fortsetzbaren machen dort weiter, wo sie waren, die übrigen müssen neu gemacht werden. Nötig, um ein gerade geändertes Relay oder einen geänderten Ordner anzuwenden.",
  "settings.restarting": "Daemon startet neu",
  "settings.restartingDetail": "Er kommt in ein paar Sekunden von allein zurück.",
  "settings.refreshing": "Wird aktualisiert…",

  "send.modeContact": "An einen Kontakt",
  "send.modeCode": "Code",
  "send.modeLink": "Link",
  "send.modeTicket": "Ticket",
  "send.blurbContact":
    "Geht direkt an jemanden aus Ihrem Adressbuch. Ist die Person verbunden, läuft es direkt von Gerät zu Gerät; ist sie es nicht, bleibt es in ihrem Postfach auf dem Relay, bis sie es abholt.",
  "send.blurbCode":
    "Ein kurzer Code zum Vorlesen oder Abscannen. Wer ihn bekommt, fügt ihn in Arvolo ein — die Person muss nicht im Adressbuch stehen, aber Sie müssen beide gerade verbunden sein.",
  "send.blurbLink":
    "Eine Adresse, die sich in jedem Browser öffnet: wer sie bekommt, braucht weder Arvolo noch ein Konto. Die Datei wird im Browser entschlüsselt, der Schlüssel reist im URL-Fragment und erreicht das Relay nie.",
  "send.blurbTicket":
    "Ein arvc…-Peer-to-Peer-Ticket: es läuft weder über das Postfach noch über das Arvolo-Relay. Um NAT zu durchstoßen, kann ein Verbindungsrelay nötig sein; es sieht nur verschlüsselten Verkehr.",
  "send.ttl1h": "1 Stunde",
  "send.ttl1d": "1 Tag",
  "send.ttl7d": "7 Tage",
  "send.ttl30d": "30 Tage",
  "send.pickerEmpty":
    "Sie haben noch niemanden im Adressbuch. Fügen Sie unter Personen jemanden hinzu — am schnellsten über den Code-Tausch, der Sie beide speichert und gleich verifiziert.",
  "send.pickerSearch": "Kontakt suchen…",
  "send.pickerRecipient": "Empfänger",
  "send.pickerNoMatch": (q) => `Kein Kontakt passt zu „${q}“.`,
  "send.depositResult": (to) =>
    `Für ${to} abgelegt. Das Ticket unten ist Ihre eigene Kopie: Sie brauchen es nur, wenn Sie es selbst übergeben wollen — etwa, wenn ${to} das Angebot nicht erhält.`,
  "send.onItsWay": (to) => `Unterwegs zu ${to}`,
  "send.onItsWayDetail":
    "Ist die Person online, geht es direkt, sonst bleibt es in ihrem Postfach.",
  "send.codeKeepDetail":
    "Der Code bleibt für mehrere Empfänger gültig, bis Sie den Versand abbrechen.",
  "send.codeOnceDetail":
    "Der Code gilt für einen einzigen Empfänger und zieht sich dann selbst zurück.",
  "send.linkDetail":
    "Wer diese Adresse hat, kann die Datei laden, bis sie abläuft, bis die erlaubten Downloads aufgebraucht sind, oder bis Sie sie unter „Links und Ablagen“ zurückziehen.",
  "send.ticketDetail":
    "Peer-to-Peer-Ticket: gültig, solange der Daemon läuft und der Versand nicht abgebrochen wurde.",
  "send.countOne": "1 Element",
  "send.countMany": (n) =>
    `${n} Elemente · sie werden in ein Archiv gepackt`,
  "send.titleReady": "Fertig",
  "send.title": "Senden",
  "send.subtitleReady": "Geben Sie weiter, was Sie unten sehen.",
  "send.subtitle": "Immer Ende-zu-Ende verschlüsselt.",
  "send.submitDeposit": "Ablegen",
  "send.submitSend": "Senden",
  "send.submitCode": "Code erzeugen",
  "send.submitLink": "Link erstellen",
  "send.submitTicket": "Ticket erstellen",
  "send.linkKeyNote":
    "Der Link trägt den Schlüssel hinter dem #: Browser schicken diesen Teil nicht an den Server, das Relay verwahrt also nur Bytes, die es nicht lesen kann.",
  "send.filesLabel": "Was Sie senden",
  "send.filesHint":
    "Sie können Dateien und Ordner auch ins Fenster ziehen.",
  "send.filesRemove": (name) => `${name} entfernen`,
  "send.pickFiles": "Dateien…",
  "send.pickFolder": "Ordner…",
  "send.whoLabel": "An wen es geht",
  "send.modeLabel": "Versandart",
  "send.noteLabel": "Zwei Zeilen für die Gegenseite (optional)",
  "send.noteHint":
    "Reisen im versiegelten Angebot mit: das Relay sieht sie nicht.",
  "send.notePlaceholder": "Hier sind die Dateien, von denen wir sprachen.",
  "send.keepCodeTitle": "Gilt für mehrere Personen",
  "send.keepCodeDesc":
    "Normalerweise gilt der Code für einen Empfänger und zieht sich dann zurück. Schalten Sie das ein, um ihn offen zu lassen, bis Sie den Versand abbrechen.",
  "send.keepCodeLabel": "Code für mehrere Personen gültig",
  "send.depositTitle": "Ins Postfach legen, nicht warten",
  "send.depositDesc":
    "Legt sofort auf dem Relay ab, auch wenn die Gegenseite verbunden ist: Sie schließen das Fenster und vergessen es. Schaltet Ablauf, Zahl der Abholungen und Passwort frei.",
  "send.depositLabel": "Ins Postfach legen",
  "send.expiresAfter": "Läuft ab nach",
  "send.depositTtlLabel": "Ablauf der Ablage",
  "send.linkTtlLabel": "Ablauf des Links",
  "send.maxPickupsLabel": "Erlaubte Abholungen",
  "send.maxPickupsHint":
    "Normalerweise nur eine: sobald geladen wurde, löscht das Relay sie.",
  "send.passwordLabel": "Passwort (optional)",
  "send.passwordHint":
    "Verschlüsselt die Ablage auch gegen die Empfängerseite: ohne dieses Passwort geht sie nicht auf. Das Relay kennt es nicht und kann es nicht wiederherstellen — verlieren Sie es, ist die Datei verloren.",
  "send.passwordPlaceholder": "keines",
  "send.linkTooMany":
    "Ein Link veröffentlicht genau ein Element. Wählen Sie eines, oder packen Sie alles in einen Ordner und wählen Sie den.",
  "send.maxDownloadsLabel": "Erlaubte Downloads",
  "send.maxDownloadsHint": "Leer lassen für kein Limit.",
  "send.maxDownloadsPlaceholder": "unbegrenzt",
  "send.noRelay":
    "Diese Versandart braucht ein Relay, und es scheint keines konfiguriert zu sein. Richten Sie eines unter Einstellungen ein.",
  "send.noArvoloRelay": "Kein Arvolo-Relay",

  "receive.explainEmpty":
    "Fügen Sie einen Sendecode ein (etwa 4821-crater-mango) oder ein arvc… / arvm…-Ticket. Um mit jemandem Kontakte zu tauschen, nehmen Sie stattdessen Personen → Ich habe einen Code.",
  "receive.explainCode":
    "Sendecode: ich verbinde mich mit dem, der ihn gerade zeigt, und lade herunter, was er schickt.",
  "receive.explainChunk":
    "Peer-to-Peer-Ticket: ich lade direkt von der Gegenseite.",
  "receive.explainMailbox":
    "Postfach-Ticket: ich hole die auf dem Relay abgelegte Datei.",
  "receive.explainUnknown":
    "Diese Form erkenne ich nicht. Ich versuche es trotzdem — der Daemon ist genauer als ich — aber prüfen Sie, ob Sie sie vollständig kopiert haben.",
  "receive.title": "Empfangen",
  "receive.subtitle": "Fügen Sie ein, was man Ihnen gegeben hat.",
  "receive.submit": "Empfangen",
  "receive.fieldLabel": "Code oder Ticket",
  "receive.passwordLabel": "Passwort (nur wenn geschützt)",
  "receive.passwordHint":
    "Wer es geschickt hat, wird es Ihnen getrennt gesagt haben. Ohne das geht eine geschützte Ablage nicht auf.",
  "receive.passwordPlaceholder": "keines",
  "receive.whereLabel": "Wohin damit",
  "receive.whereHint": (dir) => `Standardordner: ${dir}`,
  "receive.whereAria": "Zielordner",
  "receive.choose": "Wählen…",
  "receive.useDefault": "Standard",
  "receive.started": "Empfang gestartet",
  "receive.startedDetail":
    "Sie finden ihn unter den eingehenden Übertragungen.",

  "incoming.unknownSender": "Unbekannter Absender",
  "incoming.started": "Empfang gestartet",
  "incoming.title": "Jemand schickt Ihnen eine Datei",
  "incoming.subtitle": "Nehmen Sie nur an, wenn Sie wissen, von wem sie kommt.",
  "incoming.reject": "Ablehnen",
  "incoming.later": "Ich entscheide später",
  "incoming.accept": "Annehmen und laden",
  "incoming.notInBook": "Nicht im Adressbuch",
  "incoming.claimedName": (name) =>
    `nennt sich „${name}“ — ein selbst gewählter Name, nichts bürgt dafür`,
  "incoming.keyFingerprint": "Fingerabdruck des Schlüssels",
  "incoming.senderId": "Öffentliche ID des Absenders",
  "incoming.hintVerified":
    "Sie haben diesen Fingerabdruck bereits außerhalb des Kanals verglichen: es ist derselbe Schlüssel, den Sie verifiziert haben.",
  "incoming.hintKnown":
    "Vergleichen Sie ihn mündlich mit der Person, die Ihnen die Datei schickt. Nur so sind Sie sicher, dass sie es wirklich ist — ein Name beweist das nicht.",
  "incoming.hintUnknown":
    "Das ist kein Fingerabdruck: es ist die rohe ID von jemandem, den Sie nicht im Adressbuch haben. Speichern Sie ihn unten, dann sehen Sie die Wörter, die Sie mündlich vergleichen können.",
  "incoming.attachedNote": "Beigelegte Nachricht",
  "incoming.passwordLabel": "Passwort",
  "incoming.passwordHint":
    "Diese Datei ist geschützt: ohne das Passwort geht sie nicht auf. Wer sie geschickt hat, wird es Ihnen getrennt gesagt haben — es reist nicht mit der Datei, und das Relay kennt es nicht.",
  "incoming.ifYouKnowThem": "Wenn Sie die Person kennen",
  "incoming.saveAsPlaceholder": "Ins Adressbuch speichern als…",
  "incoming.saveAsLabel": "Name für den Kontakt",
  "incoming.savedAs": (name) => `Als ${name} gespeichert`,
  "incoming.savedAsDetail":
    "Bleibt unverifiziert: bestätigen Sie den Fingerabdruck mündlich und markieren Sie ihn dann unter Personen.",
  "incoming.saveNote":
    "Speichern verifiziert nicht. Verifiziert wird die Person erst, wenn Sie den Fingerabdruck persönlich oder mündlich vergleichen.",
  "incoming.blockAndReject": "Sperren und ablehnen",
  "incoming.blocked": "Gesperrt",
  "incoming.blockedDetail":
    "Angebote dieser Person werden bei Ankunft verworfen, ohne Hinweis an Sie.",

  "pair.titleContact": "Kontakte tauschen",
  "pair.titleDeviceHost": "Ein weiteres eigenes Gerät verbinden",
  "pair.titleDeviceJoin": "Dieses Gerät verbinden",
  "pair.subContactHost":
    "Zeigen Sie den Code: Sie speichern einander, schon verifiziert.",
  "pair.subContactJoin": "Geben Sie den Code ein, den man Ihnen vorgelesen hat.",
  "pair.subDeviceHost": "Teilt Ihre Identität mit dem neuen Rechner.",
  "pair.subDeviceJoin":
    "Ersetzt die Identität dieses Geräts durch die gemeinsame.",
  "pair.restarting": "Daemon startet neu",
  "pair.restartingDetail":
    "Er fährt mit der gemeinsamen Identität wieder hoch: ein paar Sekunden, dann ist alles zurück.",
  "pair.restartAndClose": "Neu starten und schließen",
  "pair.link": "Verbinden",
  "pair.done": "Fertig",
  "pair.needsRestart":
    "Der Daemon läuft noch mit der vorherigen Identität. Er muss neu gestartet werden, damit die Änderung wirkt — der Knopf unten erledigt das.",
  "pair.failed": "Das hat nicht geklappt",
  "pair.deviceWarnLead": "Das teilt Ihre geheime Identität.",
  "pair.deviceWarnRest":
    "Wer den Code eingibt, wird zu Ihnen: gleiche öffentliche ID, gleiches Postfach, gleiches Adressbuch. Nur auf einem eigenen Rechner benutzen. Der Code gilt für ein Gerät und verfällt, sobald er benutzt wurde.",
  "pair.captionDevice":
    "Auf dem anderen Gerät: Ihre Geräte → Ich habe einen Code.",
  "pair.captionContact":
    "Lesen Sie ihn vor. Die Gegenseite öffnet Personen → Ich habe einen Code und gibt ihn ein.",
  "pair.waitingOther":
    "Warte auf die andere Seite… zum Abbrechen einfach schließen.",
  "pair.preparingCode": "Ich bereite den Code vor…",
  "pair.contactNote":
    "Getauscht werden nur die öffentlichen IDs. Ihre geheime Identität und Ihr Adressbuch verlassen diesen Rechner nicht.",
  "pair.joinWarnLead": "Achtung: das lässt sich nicht rückgängig machen.",
  "pair.joinWarnRest":
    "Die aktuelle Identität dieses Geräts wird durch die gemeinsame ersetzt. Alles, was noch für die alte Identität versiegelt ist, lässt sich hier nicht mehr öffnen.",
  "pair.codeLabel": "Code",
  "pair.codeHint":
    "Der auf dem anderen Rechner angezeigte, etwa 4821-crater-mango.",
  "pair.nameLabel": "Wie Sie die Person nennen (optional)",
  "pair.nameHint":
    "Lassen Sie es leer, speichere ich sie unter einem Namen aus ihrem Fingerabdruck; umbenennen können Sie jederzeit.",
  "pair.understood":
    "Verstanden: dieses Gerät verliert seine bisherige Identität.",
  "pair.waitingMachine": "Warte auf den anderen Rechner…",
  "pair.cancelled": "Abgebrochen.",

  "palette.groupGoTo": "Gehe zu",
  "palette.groupActions": "Aktionen",
  "palette.groupPeople": "Personen",
  "palette.send": "Dateien senden…",
  "palette.sendHint": "Kontakt, Code, Link oder Ticket",
  "palette.sendKw": "senden schicken upload neu teilen",
  "palette.receive": "Empfangen…",
  "palette.receiveHint": "einen Code oder ein Ticket einfügen",
  "palette.receiveKw": "herunterladen download einfuegen",
  "palette.pairContact": "Mit jemandem Kontakte tauschen",
  "palette.pairContactHint": "Sie speichern einander, schon verifiziert",
  "palette.pairContactKw": "pairing koppeln person hinzufuegen verifizieren",
  "palette.pairDevice": "Ein weiteres eigenes Gerät verbinden",
  "palette.pairDeviceKw": "multidevice identitaet abgleich",
  "palette.sync": "Adressbuch jetzt abgleichen",
  "palette.syncKw": "kontakte geraete",
  "palette.resumeAll": "Alle Übertragungen fortsetzen",
  "palette.pauseAll": "Alle Übertragungen pausieren",
  "palette.pauseAllKw": "pause alle anhalten stoppen fortsetzen",
  "palette.clearFinished": "Abgeschlossene Übertragungen aufräumen",
  "palette.clearFinishedKw": "leeren fertige",
  "palette.navTransfersKw": "board sendungen",
  "palette.navPeopleKw": "kontakte adressbuch",
  "palette.navDepositsKw": "relay zurueckziehen",
  "palette.navHistoryKw": "protokoll log vergangenheit",
  "palette.navDevicesKw": "abgleich identitaet",
  "palette.navSettingsKw": "config relay name",
  "palette.themeLight": "Zum hellen Erscheinungsbild wechseln",
  "palette.themeSystem": "Dem System folgen",
  "palette.themeDark": "Zum dunklen Erscheinungsbild wechseln",
  "palette.themeKw": "thema dunkel hell dark light darstellung",
  "palette.sendTo": (name) => `An ${name} senden`,
  "palette.verified": "verifiziert",
  "palette.notVerified": "nicht verifiziert",
  "palette.openCard": (name) => `Karte von ${name} öffnen`,
  "palette.personKw": "fingerabdruck fingerprint verifizieren",
  "palette.label": "Suchen und ausführen",
  "palette.placeholder": "Befehl oder Person suchen…",
  "palette.noMatch": (q) => `Nichts passt zu „${q}“.`,

  "store.unknownPeer": "unbekannt",
  "store.loadTransfers": (e) =>
    `Ich kann die Übertragungen nicht vom Daemon lesen: ${e}`,
  "store.loadHistory": (e) =>
    `Ich kann den Verlauf nicht vom Daemon lesen: ${e}`,
  "store.loadDeposits": (e) =>
    `Ich kann die Links nicht vom Daemon lesen: ${e}`,
  "store.loadConfig": (e) =>
    `Ich kann die Einstellungen nicht vom Daemon lesen: ${e}`,
  "store.loadSync": (e) => `Ich kann den Gerätezustand nicht lesen: ${e}`,
  "store.errClearHistory": "Der Verlauf ließ sich nicht leeren",
  "store.errRevokeLink": "Der Link ließ sich nicht zurückziehen",
  "store.errSaveConfig": "Die Einstellungen ließen sich nicht speichern",
  "store.errPruneNames": "Die Namen ließen sich nicht aufräumen",
  "store.errSend": (to) => `Das Senden an ${to} ist fehlgeschlagen`,
  "store.errDeposit": (to) => `Die Ablage für ${to} ist fehlgeschlagen`,
  "store.errTicket": "Das Ticket ließ sich nicht erstellen",
  "store.errCode": "Der Code ließ sich nicht erstellen",
  "store.errLink": "Der Link ließ sich nicht erstellen",
  "store.errReceive": "Der Empfang ist fehlgeschlagen",
  "store.errAccept": "Die Datei ließ sich nicht annehmen",
  "store.errReject": "Die Datei ließ sich nicht ablehnen",
  "store.errPause": "Pausieren nicht möglich",
  "store.errResume": "Fortsetzen nicht möglich",
  "store.errCancel": "Abbrechen nicht möglich",
  "store.errRemove": "Löschen nicht möglich",
  "store.errVerify": (name) => `${name} ließ sich nicht verifizieren`,
  "store.errUnverify": (name) =>
    `Die Verifizierung von ${name} ließ sich nicht zurücknehmen`,
  "store.errTrust": (who) =>
    `${who} ließ sich nicht als vertrauenswürdig markieren`,
  "store.errUntrust": (who) =>
    `Das Vertrauen zu ${who} ließ sich nicht zurücknehmen`,
  "store.errBlock": (who) => `${who} ließ sich nicht sperren`,
  "store.errUnblock": (who) => `${who} ließ sich nicht entsperren`,
  "store.errAcceptName": (who) =>
    `Der Name von ${who} ließ sich nicht übernehmen`,
  "store.errAddContact": (name) => `${name} ließ sich nicht speichern`,
  "store.errRemoveContact": (name) => `${name} ließ sich nicht entfernen`,
  "store.errRenameContact": (old) => `${old} ließ sich nicht umbenennen`,
  "store.errSetMyName": "Der Name ließ sich nicht setzen",
  "store.errRestartDaemon": "Der Daemon ließ sich nicht neu starten",
  "store.errClearFinished":
    "Die abgeschlossenen Übertragungen ließen sich nicht aufräumen",
  "store.syncFailed": "Der Abgleich ist fehlgeschlagen",
  "store.syncOk": "Adressbuch abgeglichen",
  "store.syncMerged": (n) =>
    n === 1
      ? "1 Aktualisierung von Ihren anderen Geräten."
      : `${n} Aktualisierungen von Ihren anderen Geräten.`,
  "store.syncNone": "Keine Aktualisierungen von Ihren anderen Geräten.",
};
