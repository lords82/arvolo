// Français.
//
// Deux conventions tenues partout dans ce fichier : le vouvoiement, parce que
// l'application s'adresse à quelqu'un qu'elle ne connaît pas ; et l'espace
// insécable avant « ? », « ! » et « : », et à l'intérieur des guillemets, parce
// que sans elle la typographie française se casse en fin de ligne.

import type { Dict } from "./en";

export const fr: Dict = {
  "locale.tag": "fr",
  "locale.name": "Français",

  "common.cancel": "Annuler",
  "common.save": "Enregistrer",
  "common.done": "Terminé",
  "common.close": "Fermer",
  "common.confirm": "Confirmer",
  "common.retry": "Réessayer",
  "common.open": "Ouvrir",
  "common.remove": "Supprimer",
  "common.refresh": "Actualiser",
  "common.copy": "Copier",
  "common.copied": "Copié",
  "common.copyFailed": "Copie impossible",
  "common.loading": "Chargement…",
  "common.to": "à",
  "common.from": "de",

  "title.transfers": "Transferts",
  "title.people": "Personnes",
  "title.deposits": "Liens et dépôts",
  "title.history": "Historique",
  "title.devices": "Vos appareils",
  "title.settings": "Réglages",

  "app.disconnected":
    "Je n'arrive pas à joindre le démon. Les transferts en cours continuent, mais cette fenêtre ne les voit pas.",
  "app.versionMismatch": (daemon, gui) =>
    `Le démon en cours d'exécution est en version ${daemon}, l'application en ${gui}. Redémarrez-le pour les aligner.`,
  "app.versionUnknown": "antérieure",
  "app.restart": "Redémarrer",
  "app.offerWaiting": "Quelqu'un veut vous envoyer un fichier.",
  "app.offersWaiting": (n) => `${n} fichiers attendent votre confirmation.`,
  "app.seeOffers": "Voir",
  "app.searchPlaceholder": "Filtrer par nom ou par personne…",
  "app.searchLabel": "Filtrer les transferts",
  "app.clearFinished": (n) => `Nettoyer (${n})`,
  "app.palette": (mod) => `Chercher et exécuter (${mod}K)`,
  "app.send": "Envoyer",
  "app.sendShortcut": (mod) => `Envoyer (${mod}N)`,
  "app.dropTitle": "Déposez ici pour envoyer",
  "app.dropHint": "Vous choisirez ensuite : un contact, un code, un lien.",
  "app.actionFailed": "Ça n'a pas marché",
  "app.errHintDaemon": "Le démon ne répond pas — redémarrez-le et réessayez.",
  "app.errHintRelay": "Il faut un relais — réglez-le dans Réglages → Réseau.",
  "app.errHintPassword": "Protégé par mot de passe — demandez-le à l'expéditeur.",
  "app.errOpenSettings": "Ouvrir les réglages",

  "crash.title": "Quelque chose a cassé dans l'interface",
  "crash.body":
    "Les transferts ne s'arrêtent pas : ils continuent dans le démon, en arrière-plan. Vous pouvez reprendre où vous en étiez.",

  "rail.nav": "Navigation principale",
  "rail.meTitle": "Votre identité et vos réglages",
  "rail.meFallback": "Moi",
  "rail.noIdentity": "identité pas encore lue",
  "rail.daemonUp": "Démon connecté",
  "rail.daemonDown": "Démon injoignable",
  "rail.send": "Envoyer…",
  "rail.receive": "Recevoir…",
  "rail.sections": "Sections",
  "rail.palette": "Chercher et exécuter",

  "status.active": "En cours",
  "status.sharing": "Partagé",

  "share.title": "Fichier partagé",
  "share.stop": "Arrêter le partage",
  "share.copies": "copies récupérées",
  "share.now": "en cours de téléchargement",
  "share.lastPickup": "dernière récupération",
  "share.never": "jamais",
  "share.uploaded": "envoyés",
  "share.fromDownload": (when: string) =>
    `Vous l\u2019avez téléchargé ${when}, et votre ordinateur le met maintenant à disposition des autres.`,
  "share.seedingSetting": "Modifier ce réglage",
  "share.countsNote":
    "Des copies, pas des personnes : un ticket ne porte aucune identité, donc la même personne qui récupère deux fois compte deux fois.",
  "status.completed": "Terminé",
  "status.deposited": "Déposé",
  "status.paused": "En pause",
  "status.incoming": "À confirmer",
  "status.stalled": "En attente",
  "status.failed": "Échoué",
  "status.cancelling": "Annulation…",
  "status.cancelled": "Annulé",

  "meta.paused": "en pause",
  "meta.sharing": "disponible — personne ne le télécharge",
  "meta.sharingPeers": (n) =>
    n === 1 ? "1 personne le télécharge" : `${n} personnes le téléchargent`,
  "meta.stalled": "reprend dès que possible",
  "meta.incoming": "ouvrir pour les détails",
  "meta.deposited": "en attente du retrait par le destinataire",
  "meta.failed": "transfert échoué",

  "eta.seconds": (n) => `${n} s`,
  "eta.minutes": (n) => `${n} min`,
  "eta.hours": (n) => `${n} h`,
  "until.seconds": (n) => (n === 1 ? "1 seconde" : `${n} secondes`),
  "until.minutes": (n) => (n === 1 ? "1 minute" : `${n} minutes`),
  "until.hours": (n) => (n === 1 ? "1 heure" : `${n} heures`),
  "until.days": (n) => (n === 1 ? "1 jour" : `${n} jours`),
  "ago.moments": "il y a quelques secondes",
  "ago.minutes": (n) =>
    n === 1 ? "il y a 1 minute" : `il y a ${n} minutes`,
  "ago.hours": (n) => (n === 1 ? "il y a 1 heure" : `il y a ${n} heures`),
  "ago.days": (n) => (n === 1 ? "il y a 1 jour" : `il y a ${n} jours`),

  "section.pending": "À confirmer",
  "section.active": "En cours et en pause",
  "section.today": "Aujourd'hui",
  "section.earlier": "Plus tôt",

  "transfers.pause": "Mettre en pause",
  "transfers.resume": "Reprendre",
  "transfers.openFile": "Ouvrir le fichier",
  "transfers.openFileFailed": "Je n'arrive pas à ouvrir le fichier",
  "transfers.openFolder": "Ouvrir le dossier",
  "transfers.openFolderFailed": "Je n'arrive pas à ouvrir le dossier",
  "transfers.revokeDeposit": "Retirer le dépôt",
  "transfers.cancel": "Annuler",
  "transfers.removeRow": "Retirer de la liste",
  "transfers.verifiedIdentity": "Identité vérifiée",
  "transfers.swarm": "Transfert réparti entre plusieurs pairs",
  "transfers.peers": (n) => (n === 1 ? "1 pair" : `${n} pairs`),
  "transfers.liveCode": "code actif",
  "transfers.review": "Revoir",
  "transfers.shareDetails": "Détails du partage",
  "transfers.saveSender": "Enregistrer l'expéditeur…",
  "transfers.saveSenderTitle": "Enregistrer l'expéditeur dans vos contacts",
  "transfers.saveSenderSub": (name) =>
    `Cette personne vous a envoyé ${name} et vous n'avez pas encore de nom pour elle.`,
  "transfers.reorder": (name) =>
    `Déplacer ${name} : faites glisser, ou utilisez les flèches haut et bas`,
  "transfers.rowActions": (name) => `Actions pour ${name}`,
  "transfers.progressOf": (name) => `Progression de ${name}`,
  "transfers.confirmRevokeTitle": "Retirer le dépôt ?",
  "transfers.confirmCancelTitle": "Annuler ?",
  "transfers.confirmRevokeBody": (peer) =>
    `Le fichier est retiré du relais et l'offre retirée de la boîte de ${peer}. Cette personne ne pourra plus le télécharger.`,
  "transfers.confirmRevokePeer": "destination",
  "transfers.confirmCancelBody": (name) =>
    `« ${name} » s'arrête là. Ce qui est déjà passé est jeté : si vous recommencez, ça repart de zéro.`,
  "transfers.confirmRevokeLabel": "Retirer",
  "transfers.confirmCancelLabel": "Annuler le transfert",
  "transfers.keepGoing": "Laisser tomber",
  "transfers.outgoing": "Sortants",
  "transfers.incoming": "Entrants",
  "transfers.emptyOutTitle": "Rien en sortie",
  "transfers.emptyInTitle": "Rien en entrée",
  "transfers.emptyOutBody":
    "Glissez un fichier dans la fenêtre, ou passez par Envoyer.",
  "transfers.emptyInBody":
    "Les fichiers qu'on vous envoie apparaissent ici.",
  "transfers.emptyOutAction": "Envoyer quelque chose",
  "transfers.emptyInAction": "Coller un code",
  "transfers.firstRunTitle": "Glissez ici les fichiers à envoyer",
  "transfers.firstRunBody":
    "Ou choisissez un contact, générez un code court à lire à voix haute, ou créez un lien qui s'ouvre dans n'importe quel navigateur. Tout est chiffré de bout en bout : le relais ne voit que des octets illisibles.",
  "transfers.firstRunAction": "Envoyer quelque chose",

  "people.presenceUnknownTitle": "Je ne sais pas : le relais n'a pas répondu",
  "people.presenceUnknownLabel": "Présence inconnue",
  "people.presenceOnTitle": "Connecté en ce moment",
  "people.presenceOffTitle": "Non connecté",
  "people.presenceOn": "Connecté",
  "people.presenceOff": "Non connecté",

  "people.menuDetails": "Détails et empreinte",
  "people.menuUnverify": "Retirer la vérification",
  "people.menuVerify": "Marquer comme vérifié…",
  "people.menuUntrust": "Plus de confiance : me demander à chaque fois",
  "people.menuTrust": "Marquer comme fiable : téléchargement automatique",
  "people.menuUnblock": "Débloquer",
  "people.menuBlock": "Bloquer",
  "people.menuRemove": "Retirer du carnet d'adresses",
  "people.rowActions": (name) => `Actions pour ${name}`,
  "people.goesBy": (name) => `se présente comme « ${name} »`,
  "people.notVerified": "Non vérifié",
  "people.notVerifiedTitle": "L'empreinte n'a jamais été comparée",
  "people.wantsToBeCalled": (name) =>
    `Souhaite se faire appeler « ${name} ».`,
  "people.approve": "Approuver",
  "people.send": "Envoyer",
  "people.details": "Détails",

  "people.confirmRemoveTitle": (name) => `Supprimer ${name} ?`,
  "people.confirmRemoveBody":
    "Cette personne disparaît du carnet d'adresses avec ses marques de vérification et de confiance. Les transferts déjà faits restent dans l'historique.",
  "people.confirmForceTitle":
    "Télécharger automatiquement depuis une clé non vérifiée ?",
  "people.confirmForceBody": (name) =>
    `Les fichiers de ${name} seraient téléchargés sans rien vous demander, alors que vous n'avez jamais comparé son empreinte en personne. Si quelqu'un s'était interposé au moment où vous l'avez ajouté, c'est de cette personne que vous téléchargeriez automatiquement.`,
  "people.confirmForceFooter":
    "La bonne façon de faire : comparer l'empreinte, puis marquer le contact comme vérifié.",
  "people.confirmForceLabel": "Forcer quand même",
  "people.confirmForceCancel": "Je vérifie d'abord",

  "people.addTitle": "Ajouter par identifiant",
  "people.addSubtitle":
    "Le chemin long : il faut son identifiant public en entier.",
  "people.addNameLabel": "Comment vous l'appelez",
  "people.addNamePlaceholder": "ex. Julie",
  "people.addIdLabel": "Identifiant public",
  "people.addIdHint":
    "Il le trouve avec « arvolo me », ou dans l'écran Réglages de son application.",
  "people.addTip":
    "Bien plus simple : Échanger les contacts. Vous vous lisez un code court et vous vous retrouvez tous les deux enregistrés et déjà vérifiés, sans copier cinquante caractères.",
  "people.addSaved": (name) => `${name} enregistré`,
  "people.addSavedDetail":
    "Reste non vérifié tant que vous n'avez pas comparé l'empreinte.",

  "person.fingerprint": "Empreinte",
  "person.fingerprintHint":
    "Les mêmes mots doivent apparaître sur son écran. Comparez-les à voix haute ou en personne — pas par messagerie sur le canal qui vous a servi à échanger l'identifiant.",
  "person.publicId": "Identifiant public",
  "person.verified": "Vérifié",
  "person.verifiedBody":
    "Vous avez confirmé cette empreinte hors bande.",
  "person.unverify": "Retirer la vérification",
  "person.notVerifiedYet": "Pas encore vérifié",
  "person.notVerifiedBody":
    "Tant que vous n'avez pas comparé l'empreinte, la seule chose que vous savez est que quelqu'un vous a donné cet identifiant.",
  "person.compared": (name) =>
    `J'ai comparé l'empreinte avec ${name} en dehors de cette application.`,
  "person.markVerified": "Marquer comme vérifié",
  "person.rename": "Renommer",
  "person.renameHint":
    "Le nom vous appartient : la clé et les marques ne bougent pas.",

  "people.swap": "Échanger les contacts",
  "people.haveCode": "J'ai un code d'appairage",
  "people.byId": "Par identifiant",
  "people.export": "Exporter",
  "people.import": "Importer",
  "people.whoIsOnline": "Qui est là",
  "people.whoIsOnlineTitle":
    "Demander au relais qui est connecté en ce moment",
  "people.moreActions": "Autres actions sur le carnet d'adresses",
  "people.prune": "Nettoyer les noms orphelins",
  "people.pruneNone": "Rien à nettoyer",
  "people.pruneOne": "1 enregistrement supprimé",
  "people.pruneMany": (n) => `${n} enregistrements supprimés`,
  "people.pruneDetail":
    "C'étaient des noms annoncés par des contacts que vous n'avez plus.",
  "people.filterLabel": "Filtre du carnet d'adresses",
  "people.filterAll": "Tous",
  "people.filterVerified": "Vérifiés",
  "people.filterTrusted": "De confiance",
  "people.filterBlocked": "Bloqués",
  "people.filterBlockedN": (n) => `Bloqués (${n})`,
  "people.searchPlaceholder": "Chercher par nom ou identifiant…",
  "people.searchLabel": "Chercher dans le carnet d'adresses",
  "people.emptyNone": "Personne dans le carnet d'adresses",
  "people.emptyNoMatch": "Aucun contact ne correspond",
  "people.emptyNoneBody":
    "Le plus rapide pour ajouter quelqu'un est de lui lire un code court : vous vous enregistrez mutuellement et vous êtes vérifiés d'emblée, sans recopier d'identifiant à la main.",
  "people.emptyNoMatchBody": "Essayez un autre filtre ou une autre recherche.",
  "people.exportFilename": "arvolo-contacts.json",
  "people.exportedOne": "1 contact exporté",
  "people.exportedMany": (n) => `${n} contacts exportés`,
  "people.exportDetail":
    "Le fichier ne contient que des identifiants publics : aucun secret.",
  "people.exportFailed": "Export impossible",
  "people.importedOne": "1 contact importé",
  "people.importedMany": (n) => `${n} contacts importés`,
  "people.importDetail": (skipped) =>
    `${skipped ? `${skipped} ignorés. ` : ""}Tous non vérifiés : les marques ne s'importent pas, parce que ces empreintes, ce n'est pas vous qui les avez contrôlées.`,
  "people.importFailed": "Import impossible",
  "people.importNotAList": "le fichier n'est pas une liste",

  "trust.blocked": "Bloqué",
  "trust.blockedTitle": "Ses offres sont écartées à l'arrivée",
  "trust.verified": "Vérifié",
  "trust.verifiedTitle": "Empreinte confirmée hors bande",
  "trust.trusted": "De confiance",
  "trust.trustedTitle": "Ses fichiers se téléchargent sans rien demander",

  "deposit.expired": "Expiré",
  "deposit.expiredDetail": "l'échéance est passée",
  "deposit.taken": "Récupéré",
  "deposit.takenDetail": "le destinataire l'a téléchargé",
  "deposit.offerPending": "il n'est pas encore arrivé chez le destinataire",
  "deposit.offerArrived": "arrivé sur son appareil, pas encore récupéré",
  "deposit.gone": "Plus disponible",
  "deposit.goneLink": "téléchargé jusqu'à la limite, ou déjà retiré",
  "deposit.goneSealed": "retiré par le destinataire, ou déjà retiré par vous",
  "deposit.expiresIn": (until) => `expire dans ${until}`,
  "deposit.expiredJustNow": "échéance tout juste passée",
  "deposit.unknown": "État inconnu",
  "deposit.unknownDetail": (when) => `relais injoignable · ${when}`,
  "deposit.downloads": (n, cap) => `${n}${cap} téléchargements`,
  "deposit.noLimit": "aucune limite",
  "deposit.max": (label) => `max ${label}`,
  "deposit.active": "Actif",

  "deposits.openInBrowser": "Ouvrir dans le navigateur",
  "deposits.openFailed": "Je n'arrive pas à ouvrir le lien",
  "deposits.share": "Link",
  "deposits.shareTicket": "Code de retrait",
  "deposits.shareTicketTitle": "Le ticket, à nouveau",
  "deposits.ticketDetail":
    "Collez-le au destinataire : il s'ouvre dans son Arvolo, ou avec `arvolo recv`. Lui seul peut le déchiffrer — il est scellé à son identité.",
  "deposits.shareTitle": "Le lien, à nouveau",
  "deposits.publicLink": "Lien public",
  "deposits.sealed": "Dépôt",
  "deposits.revoke": "Retirer",
  "deposits.sealedFor": (who, detail) => `scellé pour ${who} · ${detail}`,
  "deposits.confirmRevokeTitle": "Retirer ?",
  "deposits.confirmRemoveTitle": "Supprimer la ligne ?",
  "deposits.confirmRevokeLink":
    "Le lien cesse de fonctionner pour tous ceux à qui vous l'avez donné, et qui l'a déjà téléchargé garde sa copie. Le fichier reste sur votre disque.",
  "deposits.confirmRevokeSealed":
    "Le fichier est retiré du relais et l'offre retirée de la boîte du destinataire. S'il ne l'a pas encore récupéré, il ne le pourra plus.",
  "deposits.confirmRemoveBody":
    "Il n'y a plus rien à retirer sur le relais : seule cette ligne disparaît.",
  "deposits.intro":
    "Ce que vous avez laissé sur un relais et pouvez encore retirer. L'état est demandé au relais chaque fois que vous ouvrez cet écran — il n'y a pas d'autre moyen de le savoir.",
  "deposits.createLink": "Créer un lien",
  "deposits.emptyTitle": "Aucun lien ni dépôt actif",
  "deposits.emptyBody":
    "Quand vous créez un lien public, ou déposez un fichier dans la boîte de quelqu'un, ça apparaît ici — et c'est d'ici que vous le retirez.",
  "deposits.sectionLinks": "Liens publics",
  "deposits.sectionSealed": "Dépôts scellés",

  "history.today": "Aujourd'hui",
  "history.yesterday": "Hier",
  "history.completed": "Terminé",
  "history.cancelled": "Annulé",
  "history.deposited": "Déposé",
  "history.failed": "Échoué",
  "history.unknownOutcome": "Issue inconnue",
  "history.filterLabel": "Filtre de l'historique",
  "history.filterAll": "Tout",
  "history.filterSent": "Envoyés",
  "history.filterReceived": "Reçus",
  "history.searchPlaceholder": "Chercher…",
  "history.searchLabel": "Chercher dans l'historique",
  "history.clear": "Vider",
  "history.emptyNoMatch": "Aucun résultat",
  "history.emptyNothing": "Rien pour l'instant",
  "history.emptyNoMatchBody": "Essayez un autre filtre ou une autre recherche.",
  "history.emptyNothingBody":
    "Chaque transfert terminé finit ici : quoi, avec qui, et comment ça s'est passé.",
  "history.confirmClearTitle": "Vider l'historique ?",
  "history.confirmClearBody":
    "Le journal est oublié en entier et ne peut pas être récupéré. Les fichiers déjà reçus restent où ils sont ; ceci n'efface que la liste.",

  "devices.identityTitle": "Votre identité partagée",
  "devices.identityHint":
    "Chaque appareil relié utilise celle-ci. Pour le reste du monde vous êtes une seule personne, où que vous ouvriez Arvolo.",
  "devices.fingerprint": "Empreinte",
  "devices.fingerprintHint":
    "Elle doit être identique sur tous vos appareils. Si une machine affiche d'autres mots, elle n'est pas reliée : c'est une autre identité.",
  "devices.publicId": "Identifiant public",
  "devices.pairTitle": "Relier un appareil",
  "devices.pairBody":
    "Le rattachement se fait des deux côtés : sur cette machine vous affichez un code, sur l'autre vous le saisissez. C'est une opération délicate — ce qui passe, c'est votre identité secrète, pas une simple invitation.",
  "devices.showCode": "Afficher un code",
  "devices.haveCode": "J'ai un code d'appairage",
  "devices.pairWarnLead": "Jamais sur une machine qui n'est pas la vôtre.",
  "devices.pairWarnRest":
    "Qui saisit le code devient vous à tous les égards : même boîte, même carnet d'adresses, même capacité à ouvrir ce qu'on vous envoie.",
  "devices.syncTitle": "Carnet d'adresses synchronisé",
  "devices.syncHint":
    "Les contacts circulent entre vos appareils dans une cellule chiffrée sur votre boîte. Le relais conserve des octets qu'il ne sait pas lire.",
  "devices.syncOn": "Active",
  "devices.syncOff": "Désactivée",
  "devices.contactCount": (n) =>
    n === 1
      ? "1 contact dans le carnet d'adresses"
      : `${n} contacts dans le carnet d'adresses`,
  "devices.lastSync": (when) => `dernière synchronisation ${when}`,
  "devices.neverSynced":
    "pas encore synchronisé depuis le démarrage du démon",
  "devices.lastError": (err) => `Le dernier tour a échoué : ${err}`,
  "devices.syncNow": "Synchroniser maintenant",
  "devices.autoTitle": "Synchroniser toute seule",
  "devices.autoDesc":
    "Le démon fait un tour toutes les quelques minutes. Si vous désactivez, le carnet d'adresses ne s'aligne que lorsque vous appuyez sur le bouton ci-dessus.",
  "devices.autoOn": "Synchronisation automatique activée",
  "devices.autoOff": "Synchronisation automatique désactivée",
  "devices.autoDetail": "Prend effet au prochain démarrage du démon.",

  "settings.sourceEnv": "imposé par la variable ARVOLO_RELAY",
  "settings.sourceConfig": "enregistré dans les réglages",
  "settings.sourceBuiltin": "par défaut, fourni avec l'application",
  "settings.sourceNone": "aucun",
  "settings.nameSaved": "Nom mis à jour",
  "settings.nameSavedDetail":
    "Il voyage dans chaque offre que vous envoyez, dès maintenant.",
  "settings.relaySaved": "Relais enregistré",
  "settings.relaySavedDetail":
    "Le démon l'utilisera à son prochain démarrage : redémarrez-le ci-dessous pour l'appliquer tout de suite.",
  "settings.whoYouAre": "Qui vous êtes",
  "settings.nameLabel": "Le nom que vous affichez",
  "settings.nameHint":
    "Il voyage dans chaque offre scellée que vous envoyez. C'est une étiquette que vous choisissez : qui la reçoit la voit entre guillemets, parce que rien ne la garantit. La seule chose qui vous identifie vraiment est l'empreinte ci-dessous.",
  "settings.namePlaceholder": "aucun",
  "settings.fingerprintLabel": "Votre empreinte",
  "settings.fingerprintHint":
    "Les mots que les autres comparent pour être sûrs que c'est bien vous. Lisez-les à voix haute quand quelqu'un vous ajoute.",
  "settings.publicIdLabel": "Votre identifiant public",
  "settings.appearance": "Apparence",
  "settings.theme": "Thème",
  "settings.themeSystem": "Système",
  "settings.themeLight": "Clair",
  "settings.themeDark": "Sombre",
  "settings.language": "Langue",
  "settings.languageAuto": "Système",
  "settings.languageHint":
    "« Système » suit la langue de votre ordinateur, et retombe sur l'anglais quand c'en est une qu'Arvolo ne parle pas.",
  "settings.startup": "Démarrage",
  "settings.autostartTitle": "Lancer à l'ouverture de session",
  "settings.autostartDesc":
    "À l'ouverture de session, Arvolo démarre caché dans la zone de notification, prêt à recevoir sans ouvrir de fenêtre. Sous Linux, l'entrée pointe vers l'emplacement actuel de l'AppImage : si vous déplacez le fichier, désactivez puis réactivez l'interrupteur.",
  "settings.autostartFailed": "Impossible de modifier le lancement automatique",
  "settings.network": "Réseau",
  "settings.relayOn": "Relais actif",
  "settings.relayOff": "Aucun relais",
  "settings.relayLabel": "Relais",
  "settings.relayLocked":
    "En ce moment c'est la variable d'environnement ARVOLO_RELAY qui décide : ce que vous écrivez ici n'aurait aucun effet tant qu'elle est définie.",
  "settings.relayHint": (current, source) =>
    `Utilisé actuellement : ${current} — ${source}. Une adresse sans schéma devient https:// ; pour un relais en clair, écrivez le schéma en entier, du type http://relay.local:6282.`,
  "settings.relayNone": "aucun",
  "settings.relayPlaceholder": "relais.exemple.fr",
  "settings.relayNote":
    "Le relais achemine les codes, la boîte et les liens. Il ne voit jamais vos fichiers en clair : ce qu'il conserve est chiffré avec des clés qu'il n'a pas.",
  "settings.files": "Fichiers",
  "settings.downloadDirLabel": "Où atterrissent les fichiers reçus",
  "settings.downloadDirEnv": "Décidé par la variable ARVOLO_DOWNLOAD_DIR.",
  "settings.downloadDirHint":
    "Vaut pour ce que vous acceptez sans choisir un dossier à la volée.",
  "settings.change": "Changer",
  "settings.dirUpdated": "Dossier mis à jour",
  "settings.dirUpdatedDetail":
    "Le démon l'utilisera à son prochain démarrage.",
  "settings.cannotOpen": "Je n'arrive pas à l'ouvrir",
  "settings.seedTitle": "Continuer à partager ce que vous avez téléchargé",
  "settings.seedDesc":
    "Laisser le seeding actif aide ceux qui téléchargent le même fichier. Vous pouvez l'éteindre si vous préférez ne pas rester dans l'essaim.",
  "settings.saved": "Réglage enregistré",
  "settings.savedDetail": "Prend effet au prochain démarrage du démon.",
  "settings.advanced": "Avancé",
  "settings.configFileLabel": "Fichier de configuration",
  "settings.configFileHint":
    "Tout ce qui n'apparaît pas ici — dossier temporaire, relais NAT, niveau de log — se règle à la main dans ce fichier, qui est commenté ligne par ligne.",
  "settings.identityKeyLabel": "Clé d'identité",
  "settings.identityKeyHint":
    "Votre secret. Ne le partagez pas : qui le possède devient vous. Pour utiliser Arvolo sur une autre de vos machines, il y a le rattachement d'appareils, qui le transfère chiffré.",
  "settings.versions": (daemon, gui) =>
    `Démon ${daemon} · interface ${gui}`,
  "settings.daemonProcess": (pid, exe) => `pid ${pid} · ${exe}`,
  "settings.restartDaemon": "Redémarrer le démon",
  "settings.confirmRestartTitle": "Redémarrer le démon ?",
  "settings.confirmRestartBody":
    "Les transferts en cours s'arrêtent : ceux qui sont reprenables repartent d'où ils en étaient, les autres sont à refaire depuis le début. C'est ce qui applique un relais ou un dossier que vous venez de changer.",
  "settings.restarting": "Démon en redémarrage",
  "settings.restartingDetail":
    "Il revient tout seul dans quelques secondes.",
  "settings.refreshing": "Actualisation…",

  "send.modeContact": "À un contact",
  "send.modeCode": "Code",
  "send.modeLink": "Lien",
  "send.modeTicket": "Ticket",
  "send.blurbContact":
    "Va droit à quelqu'un de votre carnet d'adresses. S'il est connecté, ça passe directement d'appareil à appareil ; sinon, ça reste dans sa boîte sur le relais jusqu'à ce qu'il le récupère.",
  "send.blurbCode":
    "Un code court à lire à voix haute ou à scanner. Qui le reçoit le colle dans Arvolo — pas besoin qu'il soit déjà dans votre carnet, mais vous devez être connectés tous les deux en ce moment.",
  "send.blurbLink":
    "Une adresse qui s'ouvre dans n'importe quel navigateur : qui la reçoit n'a besoin ni d'Arvolo ni d'un compte. Le fichier est déchiffré dans le navigateur, la clé voyage dans le fragment de l'URL et n'arrive jamais au relais.",
  "send.blurbTicket":
    "Un ticket arvc… pair-à-pair : il ne passe ni par la boîte ni par le relais Arvolo. Pour percer le NAT, un relais de connexion peut être nécessaire ; il ne voit que du trafic chiffré.",
  "send.ttl1h": "1 heure",
  "send.ttl1d": "1 jour",
  "send.ttl7d": "7 jours",
  "send.ttl30d": "30 jours",
  "send.pickerEmpty":
    "Vous n'avez encore personne dans votre carnet d'adresses. Ajoutez quelqu'un depuis Personnes — le plus rapide est l'échange par code, qui vous enregistre mutuellement et déjà vérifiés.",
  "send.pickerSearch": "Chercher un contact…",
  "send.pickerRecipient": "Destinataire",
  "send.pickerNoMatch": (q) => `Aucun contact ne correspond à « ${q} ».`,
  "send.depositResult": (to) =>
    `Déposé pour ${to}. Le ticket ci-dessous est votre copie : il ne sert que si vous voulez le remettre vous-même, par exemple si ${to} ne reçoit pas l'offre.`,
  "send.onItsWay": (to) => `En route vers ${to}`,
  "send.saveArvolo": "Enregistrer en fichier .arvolo…",
  "send.arvoloSaved": (name: string) => `${name} enregistré — partagez-le par n'importe quel canal ; en face, il s'ouvre avec Arvolo.`,
  "send.onItsWayDetail":
    "S'il est en ligne, ça passe en direct ; sinon, ça reste dans sa boîte.",
  "send.codeKeepDetail":
    "Le code reste valable pour plusieurs destinataires jusqu'à ce que vous annuliez l'envoi.",
  "send.codeOnceDetail":
    "Le code vaut pour un seul destinataire, puis se retire tout seul.",
  "send.linkDetail":
    "Quiconque possède cette adresse peut télécharger le fichier jusqu'à son expiration, jusqu'à épuisement des téléchargements autorisés, ou jusqu'à ce que vous le retiriez depuis « Liens et dépôts ».",
  "send.ticketDetail":
    "Ticket pair-à-pair : valable tant que le démon tourne et que l'envoi n'a pas été annulé.",
  "send.countOne": "1 élément",
  "send.countMany": (n) =>
    `${n} éléments · ils seront réunis dans une archive`,
  "send.titleReady": "Prêt",
  "send.title": "Envoyer",
  "send.subtitleReady": "Transmettez ce que vous voyez ci-dessous.",
  "send.subtitle": "Chiffré de bout en bout, toujours.",
  "send.submitDeposit": "Laisser dans sa boîte",
  "send.submitSend": "Envoyer",
  "send.submitCode": "Générer le code",
  "send.submitLink": "Créer le lien",
  "send.submitTicket": "Créer le ticket",
  "send.linkKeyNote":
    "Le lien porte la clé après le # : les navigateurs n'envoient pas cette partie au serveur, donc le relais ne conserve que des octets qu'il ne sait pas lire.",
  "send.filesLabel": "Ce que vous envoyez",
  "send.filesHint":
    "Vous pouvez aussi glisser fichiers et dossiers dans la fenêtre.",
  "send.filesRemove": (name) => `Retirer ${name}`,
  "send.pickFiles": "Fichiers…",
  "send.pickFolder": "Dossier…",
  "send.whoLabel": "À qui ça va",
  "send.modeLabel": "Mode d'envoi",
  "send.noteLabel": "Deux lignes pour le destinataire (facultatif)",
  "send.noteHint":
    "Ça voyage dans l'offre scellée : le relais ne le voit pas.",
  "send.notePlaceholder": "Voici les fichiers dont on parlait.",
  "send.keepCodeTitle": "Vaut pour plusieurs personnes",
  "send.keepCodeDesc":
    "Par défaut le code vaut pour un seul destinataire, puis se retire. Activez pour le laisser ouvert jusqu'à ce que vous annuliez l'envoi.",
  "send.keepCodeLabel": "Code valable pour plusieurs personnes",
  "send.depositTitle": "Laisser dans la boîte, ne pas attendre",
  "send.depositDesc":
    "Dépose tout de suite sur le relais même s'il est connecté : vous fermez et vous n'y pensez plus. Débloque l'expiration, le nombre de retraits et le mot de passe.",
  "send.depositLabel": "Laisser dans la boîte",
  "send.expiresAfter": "Expire après",
  "send.depositTtlLabel": "Expiration du dépôt",
  "send.linkTtlLabel": "Expiration du lien",
  "send.maxPickupsLabel": "Retraits autorisés",
  "send.maxPickupsHint":
    "Un seul, en principe : dès qu'il le télécharge, le relais l'efface.",
  "send.passwordLabel": "Mot de passe (facultatif)",
  "send.passwordHint":
    "Chiffre le dépôt y compris pour le destinataire : sans ce mot de passe, il ne s'ouvre pas. Le relais ne le connaît pas et ne peut pas le récupérer — si vous le perdez, le fichier est perdu.",
  "send.passwordPlaceholder": "aucun",
  "send.linkTooMany":
    "Un lien publie un seul élément. Choisissez-en un, ou mettez tout dans un dossier et sélectionnez-le.",
  "send.maxDownloadsLabel": "Téléchargements autorisés",
  "send.maxDownloadsHint": "Laissez vide pour ne pas mettre de limite.",
  "send.maxDownloadsPlaceholder": "illimités",
  "send.noRelay":
    "Ce mode a besoin d'un relais et aucun ne semble configuré. Réglez-en un depuis Réglages.",
  "send.noArvoloRelay": "Aucun relais Arvolo",

  "receive.explainEmpty":
    "Collez un code d'envoi (du type 4821-crater-mango) ou un ticket arvc… / arvm…. Pour échanger les contacts avec quelqu'un, passez plutôt par Personnes → J'ai un code.",
  "receive.explainCode":
    "Code d'envoi : je me connecte à qui l'affiche en ce moment et je télécharge ce qu'il envoie.",
  "receive.explainChunk":
    "Ticket pair-à-pair : je télécharge directement depuis l'expéditeur.",
  "receive.explainMailbox":
    "Ticket de boîte : je récupère le fichier déposé sur le relais.",
  "receive.explainUnknown":
    "Je ne reconnais pas cette forme. Je l'essaie quand même — le démon est plus précis que moi — mais vérifiez que vous l'avez copiée en entier.",
  "receive.title": "Recevoir",
  "receive.subtitle": "Collez ce qu'on vous a donné.",
  "receive.submit": "Recevoir",
  "receive.fieldLabel": "Code ou ticket",
  "receive.passwordLabel": "Mot de passe (seulement si protégé)",
  "receive.passwordHint":
    "Qui vous l'a envoyé vous l'aura dit à part. Sans lui, un dépôt protégé ne s'ouvre pas.",
  "receive.passwordPlaceholder": "aucun",
  "receive.whereLabel": "Où l'enregistrer",
  "receive.whereHint": (dir) => `Dossier par défaut : ${dir}`,
  "receive.whereAria": "Dossier de destination",
  "receive.choose": "Choisir…",
  "receive.useDefault": "Par défaut",
  "receive.started": "Réception lancée",
  "receive.startedDetail": "Vous la trouvez parmi les transferts entrants.",

  "incoming.unknownSender": "Expéditeur inconnu",
  "incoming.started": "Réception lancée",
  "incoming.title": "On vous envoie un fichier",
  "incoming.subtitle": "N'acceptez que si vous savez de qui ça vient.",
  "incoming.reject": "Refuser",
  "incoming.later": "Je décide plus tard",
  "incoming.accept": "Accepter et télécharger",
  "incoming.notInBook": "Pas dans le carnet d'adresses",
  "incoming.claimedName": (name) =>
    `se présente comme « ${name} » — un nom qu'il choisit lui-même, rien ne le garantit`,
  "incoming.keyFingerprint": "Empreinte de la clé",
  "incoming.senderId": "Identifiant public de l'expéditeur",
  "incoming.hintVerified":
    "Vous avez déjà comparé cette empreinte hors bande : c'est la même clé que vous avez vérifiée.",
  "incoming.hintKnown":
    "Comparez-la à voix haute avec la personne qui vous envoie le fichier. C'est le seul moyen d'être sûr que c'est bien elle — un nom ne le prouve pas.",
  "incoming.hintUnknown":
    "Ce n'est pas une empreinte : c'est l'identifiant brut de quelqu'un qui n'est pas dans votre carnet d'adresses. Enregistrez-le ci-dessous et vous verrez les mots à comparer à voix haute avec lui.",
  "incoming.attachedNote": "Message joint",
  "incoming.passwordLabel": "Mot de passe",
  "incoming.passwordHint":
    "Ce fichier est protégé : sans le mot de passe, il ne s'ouvre pas. Qui vous l'a envoyé vous l'aura dit à part — il ne voyage pas avec le fichier, et le relais ne le connaît pas.",
  "incoming.ifYouKnowThem": "Si vous le connaissez",
  "incoming.saveAsPlaceholder": "L'enregistrer dans le carnet comme…",
  "incoming.saveAsLabel": "Nom à donner au contact",
  "incoming.savedAs": (name) => `Enregistré sous ${name}`,
  "incoming.savedAsDetail":
    "Reste non vérifié : confirmez l'empreinte à voix haute, puis marquez-le depuis Personnes.",
  "incoming.saveNote":
    "L'enregistrer ne le vérifie pas. Il devient vérifié seulement quand vous comparez l'empreinte en personne ou à voix haute.",
  "incoming.blockAndReject": "Bloquer et refuser",
  "incoming.blocked": "Bloqué",
  "incoming.blockedDetail":
    "Ses offres seront écartées à l'arrivée, sans vous prévenir.",

  "pair.titleContact": "Échanger les contacts",
  "pair.titleDeviceHost": "Relier un autre de vos appareils",
  "pair.titleDeviceJoin": "Relier cet appareil",
  "pair.subContactHost":
    "Montrez-lui le code : vous vous enregistrez mutuellement, déjà vérifiés.",
  "pair.subContactJoin": "Saisissez le code qu'on vous a lu.",
  "pair.subDeviceHost":
    "Partage votre identité avec la nouvelle machine.",
  "pair.subDeviceJoin":
    "Remplace l'identité de cet appareil par l'identité partagée.",
  "pair.restarting": "Démon en redémarrage",
  "pair.restartingDetail":
    "Il repart avec l'identité partagée : quelques secondes et tout revient.",
  "pair.restartAndClose": "Redémarrer et fermer",
  "pair.link": "Relier",
  "pair.done": "Terminé",
  "pair.needsRestart":
    "Le démon tourne encore avec l'identité précédente. Il faut le redémarrer pour que le changement prenne effet — le bouton ci-dessous s'en charge.",
  "pair.failed": "Ça n'a pas marché",
  "pair.deviceWarnLead": "Ceci partage votre identité secrète.",
  "pair.deviceWarnRest":
    "Qui saisit le code devient vous : même identifiant public, même boîte, même carnet d'adresses. À n'utiliser que sur une machine à vous. Le code vaut pour un seul appareil et expire dès qu'il est utilisé.",
  "pair.captionDevice":
    "Sur l'autre appareil : Vos appareils → J'ai un code.",
  "pair.captionContact":
    "Lisez-le-lui. Il ouvre Personnes → J'ai un code et le saisit.",
  "pair.waitingOther":
    "En attente de l'autre partie… vous pouvez fermer pour annuler.",
  "pair.preparingCode": "Je prépare le code…",
  "pair.contactNote":
    "Seuls les identifiants publics sont échangés. Votre identité secrète et votre carnet d'adresses ne sortent pas d'ici.",
  "pair.joinWarnLead": "Attention : c'est une opération irréversible.",
  "pair.joinWarnRest":
    "L'identité actuelle de cet appareil est remplacée par l'identité partagée. Tout ce qui est encore scellé pour l'ancienne identité ne sera plus ouvrable ici.",
  "pair.codeLabel": "Code",
  "pair.codeHint":
    "Celui affiché sur l'autre machine, du type 4821-crater-mango.",
  "pair.nameLabel": "Comment vous l'appelez (facultatif)",
  "pair.nameHint":
    "Si vous laissez vide, je l'enregistre sous un nom tiré de son empreinte, et vous le renommez quand vous voulez.",
  "pair.understood":
    "J'ai compris : cet appareil perd son identité actuelle.",
  "pair.waitingMachine": "En attente de l'autre machine…",
  "pair.cancelled": "Annulé.",

  "palette.groupGoTo": "Aller à",
  "palette.groupActions": "Actions",
  "palette.groupPeople": "Personnes",
  "palette.send": "Envoyer des fichiers…",
  "palette.sendHint": "contact, code, lien ou ticket",
  "palette.sendKw": "envoi expedier upload nouveau partager",
  "palette.receive": "Recevoir…",
  "palette.receiveHint": "coller un code ou un ticket",
  "palette.receiveKw": "telecharger download coller",
  "palette.pairContact": "Échanger les contacts avec quelqu'un",
  "palette.pairContactHint": "vous vous enregistrez mutuellement, déjà vérifiés",
  "palette.pairContactKw": "pairing appairer ajouter personne verifier",
  "palette.pairDevice": "Relier un autre de vos appareils",
  "palette.pairDeviceKw": "multidevice identite synchroniser",
  "palette.sync": "Synchroniser le carnet d'adresses maintenant",
  "palette.syncKw": "contacts appareils",
  "palette.resumeAll": "Reprendre tous les transferts",
  "palette.pauseAll": "Mettre tous les transferts en pause",
  "palette.pauseAllKw": "pause tout arreter suspendre reprendre",
  "palette.clearFinished": "Nettoyer les transferts terminés",
  "palette.clearFinishedKw": "vider termines",
  "palette.navTransfersKw": "tableau envois",
  "palette.navPeopleKw": "contacts carnet adresses",
  "palette.navDepositsKw": "relais retirer",
  "palette.navHistoryKw": "journal log passe",
  "palette.navDevicesKw": "sync identite",
  "palette.navSettingsKw": "config relais nom",
  "palette.themeLight": "Passer au thème clair",
  "palette.themeSystem": "Suivre le thème du système",
  "palette.themeDark": "Passer au thème sombre",
  "palette.themeKw": "theme sombre clair dark light apparence",
  "palette.sendTo": (name) => `Envoyer à ${name}`,
  "palette.verified": "vérifié",
  "palette.notVerified": "non vérifié",
  "palette.openCard": (name) => `Ouvrir la fiche de ${name}`,
  "palette.personKw": "empreinte fingerprint verifier",
  "palette.label": "Chercher et exécuter",
  "palette.placeholder": "Chercher une commande ou une personne…",
  "palette.noMatch": (q) => `Rien ne correspond à « ${q} ».`,

  "store.unknownPeer": "inconnu",
  "store.loadTransfers": (e) =>
    `Je n'arrive pas à lire les transferts depuis le démon : ${e}`,
  "store.loadHistory": (e) =>
    `Je n'arrive pas à lire l'historique depuis le démon : ${e}`,
  "store.loadDeposits": (e) =>
    `Je n'arrive pas à lire les liens depuis le démon : ${e}`,
  "store.loadConfig": (e) =>
    `Je n'arrive pas à lire les réglages depuis le démon : ${e}`,
  "store.loadSync": (e) =>
    `Je n'arrive pas à lire l'état des appareils : ${e}`,
  "store.errClearHistory": "Impossible de vider l'historique",
  "store.errRevokeLink": "Impossible de retirer le lien",
  "store.errSaveConfig": "Impossible d'enregistrer les réglages",
  "store.errPruneNames": "Impossible de nettoyer les noms",
  "store.errSend": (to) => `L'envoi à ${to} a échoué`,
  "store.errDeposit": (to) => `Le dépôt pour ${to} a échoué`,
  "store.errTicket": "Création du ticket impossible",
  "store.errCode": "Création du code impossible",
  "store.errLink": "Création du lien impossible",
  "store.errReceive": "Réception impossible",
  "store.errAccept": "Impossible d'accepter le fichier",
  "store.errReject": "Impossible de refuser le fichier",
  "store.declined": (who: string) => who ? `Offre de ${who} refusée.` : "Offre refusée.",
  "bits.qrTooDense": "Trop de données pour un QR — utilisez Copier.",
  "store.errPause": "Impossible de mettre en pause",
  "store.errResume": "Impossible de reprendre",
  "store.errCancel": "Impossible d'annuler",
  "store.errRemove": "Impossible de supprimer",
  "store.errVerify": (name) => `Impossible de vérifier ${name}`,
  "store.errUnverify": (name) =>
    `Impossible de retirer la vérification de ${name}`,
  "store.errTrust": (who) => `Impossible de faire confiance à ${who}`,
  "store.errUntrust": (who) => `Impossible de retirer la confiance à ${who}`,
  "store.errBlock": (who) => `Impossible de bloquer ${who}`,
  "store.errUnblock": (who) => `Impossible de débloquer ${who}`,
  "store.errAcceptName": (who) => `Impossible d'approuver le nom de ${who}`,
  "store.errAddContact": (name) => `Impossible d'enregistrer ${name}`,
  "store.errRemoveContact": (name) => `Impossible de retirer ${name}`,
  "store.errRenameContact": (old) => `Impossible de renommer ${old}`,
  "store.errSetMyName": "Impossible de définir le nom",
  "store.errRestartDaemon": "Impossible de redémarrer le démon",
  "store.errClearFinished": "Impossible de nettoyer les transferts terminés",
  "store.syncFailed": "Synchronisation impossible",
  "store.syncOk": "Carnet d'adresses synchronisé",
  "store.syncMerged": (n) =>
    n === 1
      ? "1 mise à jour depuis vos autres appareils."
      : `${n} mises à jour depuis vos autres appareils.`,
  "store.syncNone": "Aucune mise à jour depuis vos autres appareils.",
};
