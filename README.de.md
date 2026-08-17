# Arvolo

*Read it in [English](README.md) · Leggilo in [italiano](README.it.md) ·
Lisez-le en [français](README.fr.md).*

**Sende Dateien an wen du willst — Ende-zu-Ende-verschlüsselt, ohne Konto,
auch wenn die Gegenseite offline ist.**

![Die Arvolo-Desktop-App](docs/assets/arvolo-app.png)

Sind beide Geräte online, reisen Dateien **peer-to-peer** — von einer Maschine
zur anderen, nie über einen Server. Ist die Empfängerin gerade weg, wartet die
Datei **versiegelt** im Postfach eines Relays: ein kleiner Server, **den du
selbst betreiben kannst** und der nur Chiffrat speichert — lesen kann er
nichts. Und für jemanden ohne installierte Software lädt ein **Link** die
Datei in jedem Browser herunter und entschlüsselt sie dort.

- **Ende-zu-Ende-verschlüsselt, immer** — Schlüssel berühren nie einen Server.
- **Kein Konto, kein Anbieter dazwischen** — deine Dateien laufen über
  Infrastruktur, die du kontrollierst.
- **Erreicht auch Offline-Empfänger** — versiegelte Ablagen warten und
  verbrennen nach dem Lesen.
- **App und Kommandozeile, ein Motor** — macOS (signiert und notarisiert),
  Windows und Linux.

## In zwei Minuten ausprobiert

**1. Hol dir Arvolo.** Lade die App aus dem [neuesten
Release](https://github.com/lords82/arvolo/releases) — `.dmg` für macOS,
`.msi` für Windows, `.AppImage` für Linux — oder installiere den
Kommandozeilen-Client:

```sh
curl -fsSL https://raw.githubusercontent.com/lords82/arvolo/main/install.sh | sh
```

**2. Zeig ihm ein Relay.** Arvolo hat keinen zentralen Server — das ist gerade
der Punkt — also braucht es ein Relay: das, das deine Firma oder ein Freund
schon betreibt, oder dein eigenes, aufgesetzt mit einem Befehl:

```sh
docker run -d --name arvolo-relay -p 6282:6282 -v arvolo-data:/data \
  ghcr.io/lords82/arvolo-relay:latest
```

In der App stellst du es unter **Einstellungen → Netzwerk** ein; auf der
Kommandozeile fragt der erste Start danach und merkt es sich.

**3. Schick etwas.** In der App: zieh eine Datei ins Fenster und teile den
kurzen Code, den sie dir gibt — die andere Person fügt ihn in ihr Arvolo ein,
und die Datei kommt an. Dasselbe im Terminal:

```sh
# du
arvolo code ./foto.jpg
#   ->  4821-crater-mango

# die andere Person
arvolo recv 4821-crater-mango
```

Drüben ist nichts installiert? `arvolo link ./bericht.pdf` druckt eine URL,
die in jedem Browser herunterlädt und entschlüsselt — ohne Installation, ohne
Konto. Das sieht, wer sie öffnet:

![Ein Arvolo-Link im Browser: die Datei wird direkt dort entschlüsselt — der
Schlüssel lebt nur im #fragment des Links, das Browser nie an den Server
schicken](docs/assets/arvolo-link-browser.png)

## Zum Weiterlesen

| | |
|---|---|
| [Das Handbuch](docs/MANUAL.md) | Jeder Befehl, jede Option, jede Einstellung — und wie es innen funktioniert. *(Englisch)* |
| [Quickstart](docs/QUICKSTART.md) | Ein Relay sauber aufsetzen, im LAN und hinter nginx + TLS. *(Englisch)* |
| [Deployment](docs/DEPLOY.md) | Self-Hosting in Produktion: systemd, Docker, Härtung. *(Englisch)* |
| [Die Desktop-App](gui/README.md) | Die GUI im Detail, mit ihrer CLI-Paritätstabelle. *(Englisch)* |
| [Das Protokoll](docs/PROTOCOL.md) | Das Wire-Format und jeder Ablauf, für Neugierige und Auditoren. *(Englisch)* |

## Lizenz

Open Core: Client und Relay sind freie Software unter
[AGPL-3.0-only](LICENSE); eine separate kommerzielle Lizenz deckt proprietäre
Nutzung und Business-Funktionen ab. „Arvolo" ist eine Marke des
Projektinhabers — siehe [CONTRIBUTING.md](CONTRIBUTING.md).
