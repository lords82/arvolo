# Arvolo

*Read it in [English](README.md) · Leggilo in [italiano](README.it.md) · Lies
es auf [Deutsch](README.de.md).*

**Envoyez des fichiers à qui vous voulez — chiffrés de bout en bout, sans
compte, même si le destinataire est hors ligne.**

![L'application de bureau Arvolo](docs/assets/arvolo-app.png)

Quand les deux appareils sont en ligne, les fichiers voyagent **en pair à
pair** — d'une machine à l'autre, jamais par un serveur. Quand le destinataire
est absent, le fichier attend **scellé** dans la boîte d'un relais : un petit
serveur **que vous pouvez héberger vous-même**, qui ne conserve que du chiffré
et ne peut rien lire. Et pour qui n'a rien d'installé, un **lien** télécharge
et déchiffre le fichier dans n'importe quel navigateur.

- **Chiffré de bout en bout, toujours** — les clés ne touchent jamais un
  serveur.
- **Pas de compte, pas d'intermédiaire** — vos fichiers passent par une
  infrastructure que vous contrôlez.
- **Atteint qui est hors ligne** — les dépôts scellés attendent le
  destinataire, puis brûlent à la lecture.
- **Application et ligne de commande, un seul moteur** — macOS (signée et
  notariée), Windows et Linux.

## Essayez-le en deux minutes

**1. Prenez Arvolo.** Téléchargez l'application depuis la [dernière
release](https://github.com/lords82/arvolo/releases) — `.dmg` pour macOS,
`.msi` pour Windows, `.AppImage` pour Linux — ou installez le client en ligne
de commande :

```sh
curl -fsSL https://raw.githubusercontent.com/lords82/arvolo/main/install.sh | sh
```

**2. Pointez-le vers un relais.** Arvolo n'a pas de serveur central — c'est
justement le principe — il lui faut donc un relais : celui que votre entreprise
ou un ami fait déjà tourner, ou le vôtre, monté en une commande :

```sh
docker run -d --name arvolo-relay -p 6282:6282 -v arvolo-data:/data \
  ghcr.io/lords82/arvolo-relay:latest
```

Dans l'application, réglez-le sous **Paramètres → Réseau** ; en ligne de
commande, le premier lancement le demande et s'en souvient.

**3. Envoyez quelque chose.** Dans l'application : glissez un fichier dans la
fenêtre et partagez le code court qu'elle vous donne — l'autre personne le
colle dans son Arvolo et le fichier arrive. Pareil depuis le terminal :

```sh
# vous
arvolo code ./photo.jpg
#   ->  4821-crater-mango

# l'autre personne
arvolo recv 4821-crater-mango
```

Rien d'installé en face ? `arvolo link ./rapport.pdf` imprime une URL qui
télécharge et déchiffre dans n'importe quel navigateur — sans installation,
sans compte. Voici ce que voit celui qui l'ouvre :

![Un lien Arvolo ouvert dans le navigateur : le fichier se déchiffre sur place
— la clé ne vit que dans le #fragment du lien, que les navigateurs n'envoient
jamais au serveur](docs/assets/arvolo-link-browser.png)

## Pour aller plus loin

| | |
|---|---|
| [Le manuel](docs/MANUAL.md) | Chaque commande, chaque option, chaque réglage — et comment ça marche dedans. *(anglais)* |
| [Quickstart](docs/QUICKSTART.md) | Monter un relais proprement, en LAN et derrière nginx + TLS. *(anglais)* |
| [Déploiement](docs/DEPLOY.md) | Auto-hébergement en production : systemd, Docker, durcissement. *(anglais)* |
| [L'application de bureau](gui/README.md) | La GUI en détail, avec sa table de parité CLI. *(anglais)* |
| [Le protocole](docs/PROTOCOL.md) | Le format réseau et chaque flux, pour les curieux et les auditeurs. *(anglais)* |

## Licence

Open core : le client et le relais sont des logiciels libres sous
[AGPL-3.0-only](LICENSE) ; une licence commerciale distincte couvre l'usage
propriétaire et les fonctions business. « Arvolo » est une marque du
propriétaire du projet — voir [CONTRIBUTING.md](CONTRIBUTING.md).
