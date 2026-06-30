# arvolo — Roadmap futura (post-MVP)

> Idee progettate ma **volutamente fuori dall'MVP**, archiviate qui per le versioni successive.
> L'MVP è: *P2P-first + UN relay self-host con mailbox a scadenza + link browser + UI desktop* (CLI-first).
> Si costruisce da qui guidati da **clienti/dati reali**, non a priori. Ogni voce conserva il razionale.

---

## 1. Mobile (iOS / Android)
- Build via **UniFFI** o **flutter_rust_bridge** sopra il core Rust.
- **Share sheet** ("Condividi con arvolo" da Foto/File).
- **Ricezione in background / push** (APNs/FCM) ad app chiusa.
- ⚠️ Punto rognoso: vincoli iOS su background e trasferimenti lunghi → validare presto con uno spike dedicato.

## 2. Federazione di relay (allow-list → consorzio → pubblica)
**Costruire solo con un cliente consorzio reale.** La base tecnica è la stessa; cambia solo la *policy di ammissione*.

- **Relay = nodo iroh con identità firmata** (node-id); insieme formano un overlay.
- **Discovery** `node-id → home-relay` via **pkarr / mainline DHT** di iroh; canale di gossip leggero per annunci/blocklist; autenticazione reciproca relay-to-relay.
- **Ammissione allow-list/consortium** (default): si fanno entrare altri relay solo se noti e firmati, su invito. Revoca = rimozione dalla lista.
- **Crescita**: relay privato di team → consorzio (inviti altri relay) → **federazione pubblica** (flag) con reputazione mittente, blocklist condivise, greylisting, quote.

### 2.1 Dove riposano i dati nel cross-relay — default **sender-holds / pull**
Mittente su R_A → destinatario su R_B (stessa federazione), P2P fallito:
- Il blob **riposa sul relay del MITTENTE R_A**. Al destinatario arriva solo un **puntatore di consegna firmato** (hash + locator R_A + claim token + scadenza), via R_B o push.
- Il destinatario **tira da R_A**. **Default = pull diretto** (destinatario ↔ R_A col claim token). **R_B fa da proxy transitorio solo come fallback/opzione** per: connettività (NAT/policy), modello client uniforme (parla solo col proprio relay), o **privacy** (nascondere il destinatario a R_A). In proxy R_B fetcha-consegna e **non conserva nulla di durevole**.
- R_A è un relay sempre-acceso → il vincolo "deve essere su per il pull" è soddisfatto; a poter andare offline è solo il *client* del mittente.
- **Vantaggi**: sovranità lato mittente (il dato non finisce su relay di terzi finché non è tirato), revoca/scadenza istantanea (R_A ha l'unica copia), costo = solo l'outbound del proprio team.
- **Opzione push→R_B** (modello email, fire-and-forget durevole): scelta per-invio quando si vuole durabilità lato destinatario.
- **Effimeri** (codice/QR, niente home-relay): il dato riposa su R_A, claim token → R_A.

### 2.2 Due permessi separati (chiave per i costi)
- **(1) Federazione di routing**: col default sender-holds il tuo relay immagazzina **solo l'outbound del tuo team**; l'inbound da altre org sta sui *loro* relay (al più banda di proxy, non storage).
- **(2) Federazione di replica**: opt-in/bilaterale, condividi storage solo dove concordato (scelta "relay amici" per-invio). Routing senza replica = reach senza costo di storage aggiuntivo.
- Quota **per-relay-pari** contro abusi; blocklist condivisa; revoca relay malevoli.

## 3. Multi-destinatario
- **Dedup del corpo** via HPKE **KEM+DEM**: corpo cifrato una volta con una content-key; solo l'**incapsulamento** è per-destinatario → il relay tiene una sola copia dei chunk corpo per N destinatari, il mittente li backfilla una volta sola.
- **GC reference-counted**: il relay tiene per ogni chunk il **set dei destinatari pendenti** e lo libera solo quando il set si svuota (refcount → 0) → in multi-destinatario non butta un chunk che serve ancora a un destinatario lento. **TTL per-destinatario** sul hold (un no-show scade); solo l'**ack firmato** del destinatario libera spazio.
- **Destinatario lento** (es. A completa, B a metà): i chunk di B **restano sul relay** perché refcount>0 → B li scarica dal relay (o dal mittente se online).

## 4. Swarming multi-sorgente (stile torrent)
- Essendo content-addressed, il ricevente scarica **range BLAKE3 di iroh-blobs in parallelo da più provider** (mittente + relay + relay amici).
- **Seeding tra co-destinatari**: un destinatario che ha completato fa da sorgente aggiuntiva per gli altri (copre il caso "mittente crashato di colpo"). **Opt-in** (trattiene i chunk ciphertext) e **vincolato da policy privacy** (rivela i co-destinatari → default off se non devono conoscersi).
- ⚠️ Scatta di rado (la maggior parte degli invii è 1 destinatario) → costruire **solo se i dati d'uso** mostrano invii grandi a molti.

### 4.1 Strategia distribuzione file grande a N destinatari
> "Semina ogni chunk una volta + swarm + relay seed durevole" → upload del mittente ≈ **1×** invece di N×.
- *Tutti online + connettibili + privacy ok* → swarm puro con semina disgiunta, relay a zero storage.
- *Qualcuno offline / NAT ostile / privacy stretta* → gli stessi chunk passano dal relay-hub.

## 5. Protocollo di trasferimento — selezione sorgente avanzata
- **Gestione asimmetrica dei guasti**: path diretto rotto (entrambi online) → relay iroh in inoltro live, zero storage, ritenta hole punching; ricevente giù → backfill mittente→relay; mittente giù (brusco) → il ricevente aspetta il ritorno.
- **Rarest-first**: alla ripresa, prima i chunk presenti solo sul mittente (meno ridondanti) per assicurarli contro un secondo crash.
- **Anti-doppio-invio**: durante la consegna diretta il mittente mette in pausa il backfill dei chunk che sta già passando direttamente.

## 6. Hardening & business (open-core)
- Policy: **download-once, revoca, password sul link, max download**.
- **Self-host packaging** (Docker/compose) + relay iroh self-hostato per controllo/privacy al 100%.
- Audit log, SSO/SAML, console admin, fatturazione.
- **Hosting gestito** a pagamento (prezzo che copre da sé storage+banda + margine).
- **Audit di sicurezza** del protocollo/crypto (HPKE auth mode, manifest firmato BLAKE3, claim token).

---

### Modello di monetizzazione (promemoria)
**Open-core / self-host**: app + relay gratis e self-hostabili → **zero costi server a nostro carico**. Ricavi da: versione **Business** (federazione, audit, SSO, console admin), **supporto**, e (dopo) **hosting gestito** a pagamento. L'adozione consumer gratuita fa da marketing; i ricavi vengono dal business che self-hosta. Niente tier gratuito illimitato ospitato da noi. Modello Bitwarden/GitLab/Mattermost.
